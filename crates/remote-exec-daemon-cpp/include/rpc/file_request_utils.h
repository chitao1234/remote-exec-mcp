#pragma once

#include <cstdint>
#include <stdexcept>
#include <string>

struct HttpRequest;
struct PathResolutionContext;

// Wire error codes shared with the Rust daemon's file tools. Names match
// remote-exec-proto's RpcErrorCode wire values.
enum class FileRpcCode {
    InvalidRequest,
    SandboxDenied,
    Missing,
    NotFile,
    DecodeFailed,
    TooLarge,
    OldStringNotFound,
    OldStringAmbiguous,
    ReadFailed,
    WriteFailed,
    Internal,
};

class FileFailure : public std::runtime_error {
public:
    FileFailure(FileRpcCode code, const std::string& message);

    FileRpcCode code;
};

const char* file_error_code_name(FileRpcCode code);
int file_error_status(FileRpcCode code);

// Byte limits and line windows arrive as JSON numbers that the Rust daemon
// treats as u64. Parse them without narrowing to the platform long width.
struct FileReadRequestSpec {
    std::string path;
    bool has_offset;
    std::uint64_t offset;
    std::uint64_t limit;
    std::uint64_t max_bytes;
};

struct FileWriteRequestSpec {
    std::string path;
    std::string content;
    std::uint64_t max_bytes;
};

struct FileEditRequestSpec {
    std::string path;
    std::string old_string;
    std::string new_string;
    bool replace_all;
    std::uint64_t max_bytes;
};

FileReadRequestSpec prepare_file_read_request(const HttpRequest& request);
FileWriteRequestSpec prepare_file_write_request(const HttpRequest& request);
FileEditRequestSpec prepare_file_edit_request(const HttpRequest& request);
