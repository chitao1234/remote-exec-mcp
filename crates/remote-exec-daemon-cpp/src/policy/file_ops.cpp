#include "policy/file_ops.h"

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <sstream>
#include <string>
#include <vector>

#include "core/logging.h"
#include "core/stdio_retry.h"
#include "core/text_file.h"
#include "platform/path_utils.h"
#include "platform/scoped_file.h"
#include "policy/path_policy.h"
#include "rpc/file_request_utils.h"
#include "rpc/server_request_utils.h"
#include "runtime/route_context.h"

#ifdef _WIN32
#include <windows.h>
#endif

namespace {

std::string system_error_detail() {
#ifdef _WIN32
    const unsigned long error = GetLastError();
    std::ostringstream out;
    out << "win32 error " << error;
    return out.str();
#else
    const int error = errno;
    std::ostringstream out;
    out << std::strerror(error) << " (errno " << error << ")";
    return out.str();
#endif
}

std::string read_file_bytes(const std::string& path) {
    ScopedFile input(path_utils::open_file(path, "rb"));
    if (!input.valid()) {
        throw FileFailure(
            FileRpcCode::ReadFailed,
            "unable to read file `" + path + "`: " + system_error_detail()
        );
    }

    std::string bytes;
    char buffer[8192];
    while (true) {
        const std::size_t received = stdio_retry::fread_some(input.get(), buffer, sizeof(buffer));
        if (received > 0U) {
            bytes.append(buffer, received);
        }
        if (received < sizeof(buffer)) {
            if (std::ferror(input.get()) != 0) {
                throw FileFailure(
                    FileRpcCode::ReadFailed,
                    "unable to read file `" + path + "`: " + system_error_detail()
                );
            }
            break;
        }
    }
    return bytes;
}

void write_file_bytes(const std::string& path, const std::string& content) {
    ScopedFile output(path_utils::open_file(path, "wb"));
    if (!output.valid()) {
        throw FileFailure(
            FileRpcCode::WriteFailed,
            "unable to write file `" + path + "`: " + system_error_detail()
        );
    }
    if (!content.empty()
        && !stdio_retry::fwrite_all(output.get(), content.data(), content.size())) {
        throw FileFailure(
            FileRpcCode::WriteFailed,
            "unable to write file `" + path + "`: " + system_error_detail()
        );
    }
    if (output.close() != 0) {
        throw FileFailure(
            FileRpcCode::WriteFailed,
            "unable to write file `" + path + "`: " + system_error_detail()
        );
    }
}

void require_regular_file(const std::string& path) {
    path_utils::PathMetadata metadata;
    if (!path_utils::path_metadata(path, &metadata)) {
        throw FileFailure(FileRpcCode::Missing, "unable to locate file `" + path + "`");
    }
    if (!metadata.is_regular_file) {
        throw FileFailure(FileRpcCode::NotFile, "file path `" + path + "` is not a file");
    }
}

void enforce_max_bytes(const std::string& path, std::uint64_t length, std::uint64_t max_bytes) {
    if (length > max_bytes) {
        std::ostringstream out;
        out << "file `" << path << "` is " << length << " bytes, exceeding max_read_bytes "
            << max_bytes;
        throw FileFailure(FileRpcCode::TooLarge, out.str());
    }
}

TextFile decode_text(
    const std::string& bytes,
    const TextDecodeSettings& settings,
    const std::string& path
) {
    try {
        return TextFile::from_bytes(bytes, settings.allow_encoding_autodetect);
    } catch (const TextFileError& ex) {
        switch (ex.kind()) {
        case TextFileError::READ_FAILED:
            throw FileFailure(
                FileRpcCode::ReadFailed,
                "unable to read file `" + path + "`: " + ex.what()
            );
        case TextFileError::DECODE_FAILED:
        case TextFileError::ENCODE_FAILED:
        case TextFileError::BINARY:
            throw FileFailure(
                FileRpcCode::DecodeFailed,
                "unable to decode file `" + path + "` as text: " + ex.what()
            );
        }
    }
    throw FileFailure(FileRpcCode::DecodeFailed, "unable to decode file `" + path + "` as text");
}

std::string encode_text(const TextFile& file, const std::string& text, const std::string& path) {
    try {
        return file.encode(text);
    } catch (const TextFileError& ex) {
        throw FileFailure(
            FileRpcCode::WriteFailed,
            "unable to encode updated text for file `" + path + "`: " + ex.what()
        );
    }
}

std::string resolve_relative_to_workdir(
    const PathResolutionContext& paths,
    const std::string& raw_path
) {
    return resolve_input_path_for_policy(
        host_path_policy(),
        paths.default_workdir,
        raw_path,
        paths.windows_posix_root
    );
}

std::string read_and_check(
    const PathResolutionContext& paths,
    const TextDecodeSettings& settings,
    const std::string& raw_path,
    std::uint64_t max_bytes,
    SandboxAccess access
) {
    const std::string path = resolve_relative_to_workdir(paths, raw_path);
    authorize_sandbox_path(paths, access, path);
    require_regular_file(path);

    const std::string bytes = read_file_bytes(path);
    enforce_max_bytes(path, static_cast<std::uint64_t>(bytes.size()), max_bytes);
    (void)settings;
    return bytes;
}

// Counts lines the way Rust's str::lines does: a trailing newline does not
// introduce an extra empty line, and "\n\n" is two lines.
std::uint64_t count_lines(const std::string& text) {
    std::uint64_t count = 0U;
    std::size_t start = 0U;
    while (start < text.size()) {
        const std::size_t newline = text.find('\n', start);
        if (newline == std::string::npos) {
            count += 1U;
            break;
        }
        // A trailing newline terminates the current line without starting a
        // new one; the final segment is empty in that case.
        count += 1U;
        start = newline + 1U;
    }
    return count;
}

std::uint64_t normalize_offset(bool has_offset, std::uint64_t offset) {
    if (!has_offset || offset == 0U) {
        return 1U;
    }
    return offset;
}

struct RenderedRead {
    std::string output;
    std::uint64_t lines_returned;
    std::uint64_t total_lines;
    bool eof;
};

std::vector<std::string> split_text_lines(const std::string& text) {
    std::vector<std::string> lines;
    std::size_t start = 0U;
    while (start <= text.size()) {
        const std::size_t newline = text.find('\n', start);
        if (newline == std::string::npos) {
            if (start < text.size()) {
                lines.push_back(text.substr(start));
            }
            break;
        }
        std::string line = text.substr(start, newline - start);
        if (!line.empty() && line[line.size() - 1U] == '\r') {
            line.erase(line.size() - 1U);
        }
        lines.push_back(line);
        start = newline + 1U;
    }
    return lines;
}

std::string format_u64(std::uint64_t value) {
    std::ostringstream out;
    out << value;
    return out.str();
}

RenderedRead render_read_output(
    const std::string& text,
    bool has_offset,
    std::uint64_t offset,
    std::uint64_t limit
) {
    const std::uint64_t start = normalize_offset(has_offset, offset);
    const std::vector<std::string> lines = split_text_lines(text);
    const std::uint64_t total_lines = static_cast<std::uint64_t>(lines.size());

    RenderedRead rendered;
    rendered.total_lines = total_lines;
    rendered.lines_returned = 0U;

    if (total_lines == 0U) {
        rendered.output = "(file is empty)";
        rendered.eof = true;
        return rendered;
    }

    if (start > total_lines) {
        rendered.output =
            "(offset out of range, file only has " + format_u64(total_lines) + " lines)";
        rendered.eof = true;
        return rendered;
    }

    const std::uint64_t limit_span = limit == 0U ? 0U : limit - 1U;
    const std::uint64_t unclamped_end = start + limit_span;
    const std::uint64_t end = unclamped_end < start
                                  ? total_lines
                                  : (unclamped_end < total_lines ? unclamped_end : total_lines);

    std::string body;
    for (std::uint64_t line_number = start; line_number <= end; ++line_number) {
        if (!body.empty()) {
            body.push_back('\n');
        }
        body += format_u64(line_number);
        body += ": ";
        body += lines[static_cast<std::size_t>(line_number - 1U)];
    }

    rendered.eof = end == total_lines;
    const std::string reminder =
        rendered.eof ? ("EOF reached, file has " + format_u64(total_lines) + " lines")
                     : ("showing lines " + format_u64(start) + "-" + format_u64(end) + " of "
                        + format_u64(total_lines) + " lines");

    rendered.output = body + "\n\n(" + reminder + ")";
    rendered.lines_returned = end - start + 1U;
    return rendered;
}

std::size_t count_occurrences(const std::string& haystack, const std::string& needle) {
    if (needle.empty()) {
        return 0U;
    }
    std::size_t count = 0U;
    std::size_t position = haystack.find(needle, 0U);
    while (position != std::string::npos) {
        count += 1U;
        position = haystack.find(needle, position + needle.size());
    }
    return count;
}

void replace_all_occurrences(
    std::string* haystack,
    const std::string& needle,
    const std::string& replacement
) {
    std::size_t position = haystack->find(needle, 0U);
    while (position != std::string::npos) {
        haystack->replace(position, needle.size(), replacement);
        position = haystack->find(needle, position + replacement.size());
    }
}

} // namespace

FileReadOutcome read_file_text(
    const PathResolutionContext& paths,
    const TextDecodeSettings& settings,
    const FileReadRequestSpec& request
) {
    if (request.limit == 0U) {
        throw FileFailure(FileRpcCode::InvalidRequest, "read.limit must be greater than zero");
    }
    if (request.max_bytes == 0U) {
        throw FileFailure(FileRpcCode::InvalidRequest, "read.max_bytes must be greater than zero");
    }

    const std::string bytes =
        read_and_check(paths, settings, request.path, request.max_bytes, SANDBOX_READ);
    const TextFile file = decode_text(bytes, settings, request.path);
    const RenderedRead rendered =
        render_read_output(file.text(), request.has_offset, request.offset, request.limit);

    LogMessageBuilder summary("file/read");
    summary.field("path", request.path);
    summary.field("lines_returned", rendered.lines_returned);
    summary.field("total_lines", rendered.total_lines);
    log_message(LOG_INFO, "server", summary.str());

    FileReadOutcome outcome;
    outcome.output = rendered.output;
    outcome.lines_returned = rendered.lines_returned;
    outcome.total_lines = rendered.total_lines;
    outcome.eof = rendered.eof;
    return outcome;
}

FileWriteOutcome write_file_text(
    const PathResolutionContext& paths,
    const TextDecodeSettings& settings,
    const FileWriteRequestSpec& request
) {
    if (request.max_bytes == 0U) {
        throw FileFailure(FileRpcCode::InvalidRequest, "write.max_bytes must be greater than zero");
    }

    const std::string path = resolve_relative_to_workdir(paths, request.path);
    authorize_sandbox_path(paths, SANDBOX_WRITE, path);

    path_utils::PathMetadata metadata;
    bool exists = false;
    if (path_utils::path_metadata(path, &metadata)) {
        if (!metadata.is_regular_file) {
            throw FileFailure(FileRpcCode::NotFile, "file path `" + path + "` is not a file");
        }
        exists = true;
        enforce_max_bytes(path, static_cast<std::uint64_t>(metadata.size), request.max_bytes);
    }

    // An existing file keeps its detected encoding; a new file is written as
    // UTF-8, which is what the Rust daemon does.
    const std::string content =
        exists
            ? encode_text(decode_text(read_file_bytes(path), settings, path), request.content, path)
            : request.content;
    path_utils::create_parent_directories(path);
    write_file_bytes(path, content);

    const std::uint64_t line_count = count_lines(request.content);
    LogMessageBuilder summary("file/write");
    summary.field("path", path);
    summary.field("created", exists ? 0UL : 1UL);
    summary.field("line_count", line_count);
    log_message(LOG_INFO, "server", summary.str());

    FileWriteOutcome outcome;
    outcome.created = !exists;
    outcome.line_count = line_count;
    return outcome;
}

FileEditOutcome edit_file_text(
    const PathResolutionContext& paths,
    const TextDecodeSettings& settings,
    const FileEditRequestSpec& request
) {
    if (request.max_bytes == 0U) {
        throw FileFailure(FileRpcCode::InvalidRequest, "edit.max_bytes must be greater than zero");
    }
    if (request.old_string.empty()) {
        throw FileFailure(FileRpcCode::InvalidRequest, "edit.old_string must not be empty");
    }

    const std::string path = resolve_relative_to_workdir(paths, request.path);
    authorize_sandbox_path(paths, SANDBOX_WRITE, path);
    require_regular_file(path);

    const std::string bytes = read_file_bytes(path);
    enforce_max_bytes(path, static_cast<std::uint64_t>(bytes.size()), request.max_bytes);

    const TextFile file = decode_text(bytes, settings, path);
    const std::size_t matches = count_occurrences(file.text(), request.old_string);
    if (matches == 0U) {
        throw FileFailure(
            FileRpcCode::OldStringNotFound,
            "old_string was not found in `" + path + "`"
        );
    }
    if (matches > 1U && !request.replace_all) {
        std::ostringstream out;
        out << "old_string matched " << matches << " times in `" << path
            << "`; set replace_all to replace every match";
        throw FileFailure(FileRpcCode::OldStringAmbiguous, out.str());
    }

    std::string updated = file.text();
    if (request.replace_all) {
        replace_all_occurrences(&updated, request.old_string, request.new_string);
    } else {
        const std::size_t position = updated.find(request.old_string, 0U);
        updated.replace(position, request.old_string.size(), request.new_string);
    }

    write_file_bytes(path, encode_text(file, updated, path));

    const std::uint64_t line_count = count_lines(updated);
    LogMessageBuilder summary("file/edit");
    summary.field("path", path);
    summary.field("replacements", static_cast<unsigned long>(matches));
    summary.field("line_count", line_count);
    log_message(LOG_INFO, "server", summary.str());

    FileEditOutcome outcome;
    outcome.replacements = static_cast<std::uint64_t>(matches);
    outcome.line_count = line_count;
    return outcome;
}
