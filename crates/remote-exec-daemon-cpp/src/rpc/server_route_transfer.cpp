#include <algorithm>
#include <cstdint>
#include <cstring>
#include <limits>
#include <sstream>
#include <string>
#include <vector>

#include "core/logging.h"
#include "rpc/server_contract.h"
#include "rpc/server_request_utils.h"
#include "rpc/server_route_common.h"
#include "rpc/server_route_transfer.h"
#include "rpc/transfer_http_codec.h"
#include "rpc/transfer_request_utils.h"
#include "rpc/transfer_stream_codec.h"

namespace {

class TransferStreamArchiveReader : public TransferArchiveReader {
public:
    TransferStreamArchiveReader() : offset_(0U), preface_read_(false), terminal_(false) {}

    bool read_exact_or_eof(char* data, std::size_t size) {
        read_preface();
        if (size == 0U) {
            return true;
        }

        std::size_t offset = 0;
        while (offset < size) {
            if (offset_ < data_.size()) {
                const std::size_t available = data_.size() - offset_;
                const std::size_t count = std::min<std::size_t>(available, size - offset);
                std::memcpy(data + offset, data_.data() + offset_, count);
                offset_ += count;
                offset += count;
                if (offset_ == data_.size()) {
                    data_.clear();
                    offset_ = 0U;
                }
                continue;
            }

            if (!read_next_data_frame_or_terminal()) {
                if (offset == 0U) {
                    return false;
                }
                throw TransferFailure(TransferRpcCode::TransferFailed, "truncated transfer body");
            }
        }
        return true;
    }

protected:
    virtual std::size_t read_transport(char* data, std::size_t size) = 0;

private:
    void read_preface() {
        if (preface_read_) {
            return;
        }
        std::string preface(server_contract::TRANSFER_STREAM_PREFACE_LEN, '\0');
        read_transport_exact(&preface[0], preface.size(), "transfer stream preface");
        if (preface != std::string(server_contract::TRANSFER_STREAM_PREFACE,
                                   server_contract::TRANSFER_STREAM_PREFACE_LEN)) {
            throw TransferFailure(TransferRpcCode::BadRequest, "invalid transfer stream preface");
        }
        preface_read_ = true;
    }

    void read_transport_exact(char* data, std::size_t size, const std::string& label) {
        std::size_t offset = 0U;
        while (offset < size) {
            const std::size_t received = read_transport(data + offset, size - offset);
            if (received == 0U) {
                throw TransferFailure(TransferRpcCode::TransferFailed, "transfer stream ended before " + label);
            }
            offset += received;
        }
    }

    bool read_next_data_frame_or_terminal() {
        if (terminal_) {
            return false;
        }

        for (;;) {
            unsigned char header[12];
            read_transport_exact(reinterpret_cast<char*>(header),
                                 server_contract::TRANSFER_STREAM_FRAME_HEADER_LEN,
                                 "transfer stream frame header");
            const unsigned char frame_type = header[0];
            if (header[1] != 0U) {
                throw TransferFailure(TransferRpcCode::BadRequest, "transfer stream frame flags must be zero");
            }
            if (header[2] != 0U || header[3] != 0U) {
                throw TransferFailure(TransferRpcCode::BadRequest, "transfer stream frame reserved field must be zero");
            }
            const std::uint64_t payload_len = transfer_stream::read_u64_be(header + 4);
            const std::uint64_t limit =
                frame_type == transfer_stream::FRAME_DATA ? server_contract::TRANSFER_STREAM_DATA_FRAME_MAX_BYTES
                                                          : server_contract::TRANSFER_STREAM_CONTROL_FRAME_MAX_BYTES;
            if (payload_len > limit) {
                throw TransferFailure(TransferRpcCode::BadRequest, "transfer stream frame payload is too large");
            }
            if (payload_len > static_cast<std::uint64_t>(std::numeric_limits<std::size_t>::max())) {
                throw TransferFailure(TransferRpcCode::BadRequest, "transfer stream frame payload is too large");
            }

            std::string payload(static_cast<std::size_t>(payload_len), '\0');
            if (!payload.empty()) {
                read_transport_exact(&payload[0], payload.size(), "transfer stream frame payload");
            }

            if (frame_type == transfer_stream::FRAME_DATA) {
                if (payload.empty()) {
                    continue;
                }
                data_.swap(payload);
                offset_ = 0U;
                return true;
            }
            if (frame_type == transfer_stream::FRAME_COMPLETE) {
                transfer_stream::parse_complete_payload(payload);
                terminal_ = true;
                return false;
            }
            if (frame_type == transfer_stream::FRAME_ERROR) {
                transfer_stream::throw_error_payload(payload);
            }
            throw TransferFailure(TransferRpcCode::BadRequest, "transfer stream frame type is unknown");
        }
    }

    std::string data_;
    std::size_t offset_;
    bool preface_read_;
    bool terminal_;
};

class HttpBodyTransferArchiveReader : public TransferStreamArchiveReader {
public:
    explicit HttpBodyTransferArchiveReader(HttpRequestBodyStream* body) : body_(body) {}

protected:
    std::size_t read_transport(char* data, std::size_t size) {
        try {
            return body_->read(data, size);
        } catch (const BadHttpRequest& ex) {
            throw TransferFailure(TransferRpcCode::TransferFailed, ex.what());
        }
    }

private:
    HttpRequestBodyStream* body_;
};

class StringTransferStreamArchiveReader : public TransferStreamArchiveReader {
public:
    explicit StringTransferStreamArchiveReader(const std::string* body) : body_(body), offset_(0U) {}

protected:
    std::size_t read_transport(char* data, std::size_t size) {
        if (offset_ >= body_->size()) {
            return 0U;
        }
        const std::size_t count = std::min<std::size_t>(size, body_->size() - offset_);
        std::memcpy(data, body_->data() + offset_, count);
        offset_ += count;
        return count;
    }

private:
    const std::string* body_;
    std::size_t offset_;
};

std::string read_request_body_to_string(HttpRequestBodyStream* body) {
    std::string output;
    char buffer[8192];
    for (;;) {
        const std::size_t received = body->read(buffer, sizeof(buffer));
        if (received == 0U) {
            return output;
        }
        output.append(buffer, received);
    }
}

void log_transfer_export_success(const TransferExportRequestSpec& export_request, const ExportedPayload& payload) {
    log_message(LOG_INFO,
                "server",
                "transfer/export path=`" + export_request.path + "` source_type=`" +
                    transfer_source_type_wire_value(payload.source_type) + "`");
}

std::string framed_transfer_body(const std::string& archive) {
    std::string body(server_contract::TRANSFER_STREAM_PREFACE, server_contract::TRANSFER_STREAM_PREFACE_LEN);
    std::size_t offset = 0U;
    while (offset < archive.size()) {
        const std::size_t size =
            std::min<std::size_t>(server_contract::TRANSFER_STREAM_DATA_FRAME_MAX_BYTES, archive.size() - offset);
        body += transfer_stream::data_frame(archive.data() + offset, size);
        offset += size;
    }
    body += transfer_stream::complete_frame(static_cast<std::uint64_t>(archive.size()));
    return body;
}

void write_transfer_export_success(HttpResponse& response,
                                   const TransferExportRequestSpec& export_request,
                                   const ExportedPayload& payload) {
    log_transfer_export_success(export_request, payload);
    write_transfer_export_headers(response, payload);
    response.body = framed_transfer_body(payload.bytes);
}

ImportSummary run_transfer_import(const TransferImportRequestSpec& import_request,
                                  TransferArchiveReader& archive_reader) {
    return import_path_from_reader(archive_reader,
                                   import_request.metadata.source_type,
                                   import_request.destination_path,
                                   import_request.metadata.overwrite,
                                   import_request.metadata.create_parent,
                                   import_request.metadata.symlink_mode,
                                   import_request.limits,
                                   import_request.authorizer);
}

ImportSummary run_transfer_import(const TransferImportRequestSpec& import_request, const std::string& body) {
    StringTransferStreamArchiveReader archive_reader(&body);
    return run_transfer_import(import_request, archive_reader);
}

void write_transfer_import_success(HttpResponse& response,
                                   const TransferImportRequestSpec& import_request,
                                   const ImportSummary& summary) {
    log_transfer_import_summary(import_request.destination_path, summary);
    write_json(response, transfer_summary_json(summary));
}

void send_transfer_export_headers(SOCKET client, const ExportedPayload& payload, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;
    response.headers["Transfer-Encoding"] = "chunked";
    write_transfer_export_headers(response, payload);
    write_request_id_header(response, request);

    std::ostringstream out;
    out << "HTTP/1.1 200 OK\r\n";
    for (std::map<std::string, std::string>::const_iterator it = response.headers.begin(); it != response.headers.end();
         ++it) {
        out << it->first << ": " << it->second << "\r\n";
    }
    out << "\r\n";
    send_all(client, out.str());
}

void send_http_chunk(SOCKET client, const char* data, std::size_t size) {
    std::ostringstream header;
    header << std::hex << size << "\r\n";
    send_all(client, header.str());
    if (size != 0U) {
        send_all_bytes(client, data, size);
    }
    send_all(client, "\r\n");
}

void send_http_chunk(SOCKET client, const std::string& bytes) {
    send_http_chunk(client, bytes.data(), bytes.size());
}

void send_chunked_terminator(SOCKET client) {
    send_all(client, "0\r\n\r\n");
}

class ChunkedTransferStreamArchiveSink : public TransferArchiveSink {
public:
    explicit ChunkedTransferStreamArchiveSink(SOCKET client) : client_(client), archive_bytes_(0U) {}

    void send_preface() {
        send_http_chunk(client_,
                        std::string(server_contract::TRANSFER_STREAM_PREFACE,
                                    server_contract::TRANSFER_STREAM_PREFACE_LEN));
    }

    void write(const char* data, std::size_t size) {
        std::size_t offset = 0U;
        while (offset < size) {
            const std::size_t chunk_size =
                std::min<std::size_t>(server_contract::TRANSFER_STREAM_DATA_FRAME_MAX_BYTES, size - offset);
            send_http_chunk(client_, transfer_stream::data_frame(data + offset, chunk_size));
            archive_bytes_ += static_cast<std::uint64_t>(chunk_size);
            offset += chunk_size;
        }
    }

    void send_complete() {
        send_http_chunk(client_, transfer_stream::complete_frame(archive_bytes_));
        send_chunked_terminator(client_);
    }

    void send_error_payload(const std::string& payload) {
        send_http_chunk(client_, transfer_stream::error_frame(payload));
        send_chunked_terminator(client_);
    }

private:
    SOCKET client_;
    std::uint64_t archive_bytes_;
};

bool try_send_streaming_transfer_response(SOCKET client, const HttpResponse& response) {
    try {
        send_all(client, render_http_response(response));
        return true;
    } catch (const SocketSendError& ex) {
        if (ex.peer_disconnected()) {
            log_message(LOG_WARN, "server", std::string("client disconnected during send: ") + ex.what());
            return false;
        }
        log_message(LOG_ERROR, "server", std::string("send failed: ") + ex.what());
        return false;
    }
}

} // namespace

HttpResponse handle_transfer_export(AppState& state, const HttpRequest& request) {
    return handle_transfer_rpc_route("transfer/export", [&](HttpResponse& response) {
        require_transfer_stream_version(request);
        const Json body = parse_json_body(request);
        const TransferExportRequestSpec export_request = prepare_transfer_export_request(state, body);
        const ExportedPayload payload = export_path(
            export_request.path, export_request.symlink_mode, export_request.exclude, export_request.authorizer);
        write_transfer_export_success(response, export_request, payload);
    });
}

HttpResponse handle_transfer_path_info(AppState& state, const HttpRequest& request) {
    return handle_transfer_rpc_route("transfer/path-info", [&](HttpResponse& response) {
        const Json body = parse_json_body(request);
        const std::string path =
            resolve_authorized_transfer_path(state, body.at("path").get<std::string>(), SANDBOX_WRITE);
        const PathInfo info = path_info(path);
        write_json(response,
                   Json{
                       {"exists", info.exists},
                       {"is_directory", info.is_directory},
                   });
    });
}

HttpResponse handle_transfer_import(AppState& state, const HttpRequest& request) {
    return handle_transfer_rpc_route("transfer/import", [&](HttpResponse& response) {
        require_transfer_stream_version(request);
        require_transfer_stream_content_type(request);
        const TransferImportRequestSpec import_request = prepare_transfer_import_request(state, request);
        write_transfer_import_success(response, import_request, run_transfer_import(import_request, request.body));
    });
}

HttpResponse
handle_streaming_transfer_import(const AppState& state, const HttpRequest& request, HttpRequestBodyStream* body) {
    HttpResponse response;
    response.status = 200;

    if (reject_before_route(state, request, &response)) {
        return response;
    }

    return handle_transfer_rpc_route("transfer/import", [&](HttpResponse& route_response) {
        require_transfer_stream_version(request);
        require_transfer_stream_content_type(request);
        const TransferImportRequestSpec import_request = prepare_transfer_import_request(state, request);
        HttpBodyTransferArchiveReader archive_reader(body);
        write_transfer_import_success(route_response,
                                      import_request,
                                      run_transfer_import(import_request, archive_reader));
    });
}

int handle_streaming_transfer_export(const AppState& state,
                                     const HttpRequest& request_head,
                                     HttpRequestBodyStream* body,
                                     SOCKET client) {
    HttpResponse rejection;
    rejection.status = 200;
    if (reject_before_route(state, request_head, &rejection)) {
        write_request_id_header(rejection, request_head);
        send_all(client, render_http_response(rejection));
        return rejection.status;
    }

    try {
        HttpRequest request = request_head;
        require_transfer_stream_version(request);
        request.body = read_request_body_to_string(body);
        const Json body_json = parse_json_body(request);
        const TransferExportRequestSpec export_request = prepare_transfer_export_request(state, body_json);
        const ExportedPayload payload = ExportedPayload{export_request.source_type, std::string()};
        send_transfer_export_headers(client, payload, request);

        ChunkedTransferStreamArchiveSink sink(client);
        sink.send_preface();
        try {
            export_path_to_sink_as(sink,
                                   export_request.path,
                                   export_request.source_type,
                                   export_request.symlink_mode,
                                   export_request.exclude,
                                   export_request.authorizer);
            log_transfer_export_success(export_request, payload);
            sink.send_complete();
        } catch (const SandboxError& ex) {
            const std::string message = ex.what();
            log_message(LOG_WARN, "server", "transfer/export failed after stream start: " + message);
            sink.send_error_payload(transfer_stream::error_payload(TransferRpcCode::SandboxDenied, ex.what()));
        } catch (const TransferFailure& failure) {
            log_message(LOG_WARN, "server", "transfer/export failed after stream start: " + failure.message);
            sink.send_error_payload(transfer_stream::error_payload(failure));
        } catch (const SocketSendError&) {
            throw;
        } catch (const std::exception& ex) {
            const std::string message = ex.what();
            log_message(LOG_WARN, "server", "transfer/export failed after stream start: " + message);
            sink.send_error_payload(transfer_stream::error_payload(ex));
        }
        return 200;
    } catch (const SandboxError& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/export failed: " + message);
        HttpResponse response;
        response.status = 400;
        write_transfer_error_response(response, ex);
        write_request_id_header(response, request_head);
        try_send_streaming_transfer_response(client, response);
        return response.status;
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", "transfer/export failed: " + failure.message);
        HttpResponse response;
        response.status = transfer_error_status(failure.code);
        write_transfer_error_response(response, failure);
        write_request_id_header(response, request_head);
        try_send_streaming_transfer_response(client, response);
        return response.status;
    } catch (const SocketSendError&) {
        throw;
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/export failed: " + message);
        HttpResponse response;
        response.status = transfer_error_status(TransferRpcCode::Internal);
        write_transfer_internal_error_response(response, message);
        write_request_id_header(response, request_head);
        try_send_streaming_transfer_response(client, response);
        return response.status;
    }
}
