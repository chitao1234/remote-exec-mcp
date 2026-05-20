#include "rpc/transfer_stream_codec.h"

#include "http/http_helpers.h"
#include "rpc/server_contract.h"

namespace {

void write_u64_be(std::uint64_t value, char* output) {
    for (std::size_t i = 0; i < 8U; ++i) {
        output[7U - i] = static_cast<char>(value & 0xffU);
        value >>= 8U;
    }
}

TransferRpcCode rpc_code_from_wire_value(const std::string& code) {
    if (code == "bad_request") {
        return TransferRpcCode::BadRequest;
    }
    if (code == "sandbox_denied") {
        return TransferRpcCode::SandboxDenied;
    }
    if (code == "transfer_path_not_absolute") {
        return TransferRpcCode::PathNotAbsolute;
    }
    if (code == "transfer_destination_exists") {
        return TransferRpcCode::DestinationExists;
    }
    if (code == "transfer_parent_missing") {
        return TransferRpcCode::ParentMissing;
    }
    if (code == "transfer_destination_unsupported") {
        return TransferRpcCode::DestinationUnsupported;
    }
    if (code == "transfer_compression_unsupported") {
        return TransferRpcCode::CompressionUnsupported;
    }
    if (code == "transfer_source_unsupported") {
        return TransferRpcCode::SourceUnsupported;
    }
    if (code == "transfer_source_missing") {
        return TransferRpcCode::SourceMissing;
    }
    if (code == "internal_error") {
        return TransferRpcCode::Internal;
    }
    return TransferRpcCode::TransferFailed;
}

std::string complete_payload(std::uint64_t archive_bytes) {
    return Json{{"archive_bytes", archive_bytes}}.dump();
}

} // namespace

namespace transfer_stream {

std::uint64_t read_u64_be(const unsigned char* data) {
    std::uint64_t value = 0U;
    for (std::size_t i = 0; i < 8U; ++i) {
        value = (value << 8U) | static_cast<std::uint64_t>(data[i]);
    }
    return value;
}

std::string encode_frame(unsigned char frame_type, const char* payload, std::size_t payload_size) {
    std::string frame(server_contract::TRANSFER_STREAM_FRAME_HEADER_LEN, '\0');
    frame[0] = static_cast<char>(frame_type);
    frame[1] = '\0';
    frame[2] = '\0';
    frame[3] = '\0';
    write_u64_be(static_cast<std::uint64_t>(payload_size), &frame[4]);
    frame.append(payload, payload_size);
    return frame;
}

std::string encode_frame(unsigned char frame_type, const std::string& payload) {
    return encode_frame(frame_type, payload.data(), payload.size());
}

std::string data_frame(const char* payload, std::size_t payload_size) {
    return encode_frame(FRAME_DATA, payload, payload_size);
}

std::string complete_frame(std::uint64_t archive_bytes) {
    return encode_frame(FRAME_COMPLETE, complete_payload(archive_bytes));
}

std::string error_frame(const std::string& payload) {
    return encode_frame(FRAME_ERROR, payload);
}

std::string error_payload(TransferRpcCode code, const std::string& message) {
    return Json{{"code", transfer_error_code_name(code)}, {"message", message}}.dump();
}

std::string error_payload(const TransferFailure& failure) {
    return error_payload(failure.code, failure.message);
}

std::string error_payload(const std::exception& ex) {
    return error_payload(TransferRpcCode::Internal, ex.what());
}

void parse_complete_payload(const std::string& payload) {
    try {
        const Json body = Json::parse(payload);
        (void)body.at("archive_bytes").get<std::uint64_t>();
    } catch (const TransferFailure&) {
        throw;
    } catch (const std::exception& ex) {
        throw TransferFailure(TransferRpcCode::BadRequest,
                              std::string("malformed transfer stream complete frame: ") + ex.what());
    }
}

void throw_error_payload(const std::string& payload) {
    try {
        const Json body = Json::parse(payload);
        const std::string code = body.value("code", std::string("transfer_failed"));
        const std::string message = body.value("message", std::string("transfer stream error"));
        throw TransferFailure(rpc_code_from_wire_value(code), message);
    } catch (const TransferFailure&) {
        throw;
    } catch (const std::exception& ex) {
        throw TransferFailure(TransferRpcCode::BadRequest,
                              std::string("malformed transfer stream error frame: ") + ex.what());
    }
}

} // namespace transfer_stream
