#pragma once

#include <cstddef>
#include <cstdint>
#include <exception>
#include <string>

#include "rpc/rpc_failures.h"

namespace transfer_stream {

enum FrameType {
    FRAME_DATA = 0x01,
    FRAME_COMPLETE = 0x02,
    FRAME_ERROR = 0x03,
};

std::uint64_t read_u64_be(const unsigned char* data);
std::string encode_frame(unsigned char frame_type, const char* payload, std::size_t payload_size);
std::string encode_frame(unsigned char frame_type, const std::string& payload);
std::string data_frame(const char* payload, std::size_t payload_size);
std::string complete_frame(std::uint64_t archive_bytes);
std::string error_frame(const std::string& payload);
std::string error_payload(TransferRpcCode code, const std::string& message);
std::string error_payload(const TransferFailure& failure);
std::string error_payload(const std::exception& ex);
void parse_complete_payload(const std::string& payload);
void throw_error_payload(const std::string& payload);

} // namespace transfer_stream
