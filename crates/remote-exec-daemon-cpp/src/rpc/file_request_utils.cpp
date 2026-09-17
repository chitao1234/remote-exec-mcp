#include "rpc/file_request_utils.h"

#include "http/http_helpers.h"

namespace {

FileFailure bad_request(const std::string& message) {
    return FileFailure(FileRpcCode::InvalidRequest, message);
}

// Reads an unsigned JSON number as u64 without going through unsigned long,
// which is 32-bit on Windows and would silently truncate.
std::uint64_t required_u64_member(const Json& body, const std::string& key) {
    const Json::const_iterator it = body.find(key);
    if (it == body.end() || it->is_null()) {
        throw bad_request("missing `" + key + "`");
    }
    if (!it->is_number_unsigned()) {
        if (!it->is_number_integer()) {
            throw bad_request("`" + key + "` must be a non-negative integer");
        }
        const std::int64_t signed_value = it->get<std::int64_t>();
        if (signed_value < 0) {
            throw bad_request("`" + key + "` must be a non-negative integer");
        }
        return static_cast<std::uint64_t>(signed_value);
    }
    return it->get<std::uint64_t>();
}

bool optional_u64_member(const Json& body, const std::string& key, std::uint64_t* out) {
    const Json::const_iterator it = body.find(key);
    if (it == body.end() || it->is_null()) {
        return false;
    }
    *out = required_u64_member(body, key);
    return true;
}

} // namespace

FileFailure::FileFailure(FileRpcCode code_value, const std::string& message)
    : std::runtime_error(message), code(code_value) {
}

const char* file_error_code_name(FileRpcCode code) {
    switch (code) {
    case FileRpcCode::InvalidRequest:
        return "bad_request";
    case FileRpcCode::SandboxDenied:
        return "sandbox_denied";
    case FileRpcCode::Missing:
        return "file_missing";
    case FileRpcCode::NotFile:
        return "file_not_file";
    case FileRpcCode::DecodeFailed:
        return "file_decode_failed";
    case FileRpcCode::TooLarge:
        return "file_too_large";
    case FileRpcCode::OldStringNotFound:
        return "file_old_string_not_found";
    case FileRpcCode::OldStringAmbiguous:
        return "file_old_string_ambiguous";
    case FileRpcCode::ReadFailed:
        return "file_read_failed";
    case FileRpcCode::WriteFailed:
        return "file_write_failed";
    case FileRpcCode::Internal:
        return "internal_error";
    }
    return "internal_error";
}

int file_error_status(FileRpcCode code) {
    return code == FileRpcCode::Internal ? 500 : 400;
}

FileReadRequestSpec prepare_file_read_request(const HttpRequest& request) {
    try {
        const Json body = parse_json_body(request);
        FileReadRequestSpec parsed;
        parsed.path = body.at("path").get<std::string>();
        parsed.has_offset = optional_u64_member(body, "offset", &parsed.offset);
        parsed.limit = required_u64_member(body, "limit");
        parsed.max_bytes = required_u64_member(body, "max_bytes");
        return parsed;
    } catch (const FileFailure&) {
        throw;
    } catch (const Json::exception& ex) {
        throw bad_request(ex.what());
    }
}

FileWriteRequestSpec prepare_file_write_request(const HttpRequest& request) {
    try {
        const Json body = parse_json_body(request);
        FileWriteRequestSpec parsed;
        parsed.path = body.at("path").get<std::string>();
        parsed.content = body.at("content").get<std::string>();
        parsed.max_bytes = required_u64_member(body, "max_bytes");
        return parsed;
    } catch (const FileFailure&) {
        throw;
    } catch (const Json::exception& ex) {
        throw bad_request(ex.what());
    }
}

FileEditRequestSpec prepare_file_edit_request(const HttpRequest& request) {
    try {
        const Json body = parse_json_body(request);
        FileEditRequestSpec parsed;
        parsed.path = body.at("path").get<std::string>();
        parsed.old_string = body.at("old_string").get<std::string>();
        parsed.new_string = body.at("new_string").get<std::string>();
        parsed.replace_all = body.value("replace_all", false);
        parsed.max_bytes = required_u64_member(body, "max_bytes");
        return parsed;
    } catch (const FileFailure&) {
        throw;
    } catch (const Json::exception& ex) {
        throw bad_request(ex.what());
    }
}
