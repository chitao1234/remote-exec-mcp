#pragma once

#include <cstdint>
#include <string>

struct FileReadRequestSpec;
struct FileWriteRequestSpec;
struct FileEditRequestSpec;
struct PathResolutionContext;

struct FileReadOutcome {
    std::string output;
    std::uint64_t lines_returned;
    std::uint64_t total_lines;
    bool eof;
};

struct FileWriteOutcome {
    bool created;
    std::uint64_t line_count;
};

struct FileEditOutcome {
    std::uint64_t replacements;
    std::uint64_t line_count;
};

// The encoding autodetect switch is read from the daemon config exactly where
// the patch engine reads it, so one setting governs every text tool.
struct TextDecodeSettings {
    TextDecodeSettings() : allow_encoding_autodetect(false) {}

    bool allow_encoding_autodetect;
};

FileReadOutcome read_file_text(
    const PathResolutionContext& paths,
    const TextDecodeSettings& settings,
    const FileReadRequestSpec& request
);

FileWriteOutcome write_file_text(
    const PathResolutionContext& paths,
    const TextDecodeSettings& settings,
    const FileWriteRequestSpec& request
);

FileEditOutcome edit_file_text(
    const PathResolutionContext& paths,
    const TextDecodeSettings& settings,
    const FileEditRequestSpec& request
);
