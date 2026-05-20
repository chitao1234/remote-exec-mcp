#include "rpc/transfer_stream_io.h"

#include <algorithm>
#include <cstring>
#include <limits>
#include <sstream>

#include "rpc/server_contract.h"
#include "rpc/transfer_stream_codec.h"

namespace {

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

} // namespace

TransferStreamArchiveReader::TransferStreamArchiveReader()
    : offset_(0U), preface_read_(false), terminal_(false) {
}

bool TransferStreamArchiveReader::read_exact_or_eof(char* data, std::size_t size) {
    read_preface();
    if (size == 0U) {
        return true;
    }

    std::size_t offset = 0U;
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

void TransferStreamArchiveReader::read_preface() {
    if (preface_read_) {
        return;
    }
    std::string preface(server_contract::TRANSFER_STREAM_PREFACE_LEN, '\0');
    read_transport_exact(&preface[0], preface.size(), "transfer stream preface");
    if (preface !=
        std::string(server_contract::TRANSFER_STREAM_PREFACE, server_contract::TRANSFER_STREAM_PREFACE_LEN)) {
        throw TransferFailure(TransferRpcCode::BadRequest, "invalid transfer stream preface");
    }
    preface_read_ = true;
}

void TransferStreamArchiveReader::read_transport_exact(char* data,
                                                       std::size_t size,
                                                       const std::string& label) {
    std::size_t offset = 0U;
    while (offset < size) {
        const std::size_t received = read_transport(data + offset, size - offset);
        if (received == 0U) {
            throw TransferFailure(TransferRpcCode::TransferFailed, "transfer stream ended before " + label);
        }
        offset += received;
    }
}

bool TransferStreamArchiveReader::read_next_data_frame_or_terminal() {
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
        const std::uint64_t limit = frame_type == transfer_stream::FRAME_DATA
                                        ? server_contract::TRANSFER_STREAM_DATA_FRAME_MAX_BYTES
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

HttpBodyTransferArchiveReader::HttpBodyTransferArchiveReader(HttpRequestBodyStream* body) : body_(body) {
}

std::size_t HttpBodyTransferArchiveReader::read_transport(char* data, std::size_t size) {
    try {
        return body_->read(data, size);
    } catch (const BadHttpRequest& ex) {
        throw TransferFailure(TransferRpcCode::TransferFailed, ex.what());
    }
}

StringTransferStreamArchiveReader::StringTransferStreamArchiveReader(const std::string* body)
    : body_(body), offset_(0U) {
}

std::size_t StringTransferStreamArchiveReader::read_transport(char* data, std::size_t size) {
    if (offset_ >= body_->size()) {
        return 0U;
    }
    const std::size_t count = std::min<std::size_t>(size, body_->size() - offset_);
    std::memcpy(data, body_->data() + offset_, count);
    offset_ += count;
    return count;
}

ChunkedTransferStreamArchiveSink::ChunkedTransferStreamArchiveSink(SOCKET client)
    : client_(client), archive_bytes_(0U) {
}

void ChunkedTransferStreamArchiveSink::send_preface() {
    send_http_chunk(
        client_, std::string(server_contract::TRANSFER_STREAM_PREFACE, server_contract::TRANSFER_STREAM_PREFACE_LEN));
}

void ChunkedTransferStreamArchiveSink::write(const char* data, std::size_t size) {
    std::size_t offset = 0U;
    while (offset < size) {
        const std::size_t chunk_size =
            std::min<std::size_t>(server_contract::TRANSFER_STREAM_DATA_FRAME_MAX_BYTES, size - offset);
        send_http_chunk(client_, transfer_stream::data_frame(data + offset, chunk_size));
        archive_bytes_ += static_cast<std::uint64_t>(chunk_size);
        offset += chunk_size;
    }
}

void ChunkedTransferStreamArchiveSink::send_complete() {
    send_http_chunk(client_, transfer_stream::complete_frame(archive_bytes_));
    send_chunked_terminator(client_);
}

void ChunkedTransferStreamArchiveSink::send_error_payload(const std::string& payload) {
    send_http_chunk(client_, transfer_stream::error_frame(payload));
    send_chunked_terminator(client_);
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
