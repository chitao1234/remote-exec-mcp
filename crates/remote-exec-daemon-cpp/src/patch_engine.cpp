#include <atomic>
#include <algorithm>
#include <cctype>
#include <cstdio>
#include <cstring>
#include <functional>
#include <sstream>
#include <stdexcept>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#endif
#include <sys/stat.h>
#ifndef _WIN32
#include <unistd.h>
#endif

#include "patch_engine.h"
#include "path_policy.h"
#include "path_utils.h"
#include "platform.h"
#include "scoped_file.h"

namespace {

enum class PatchKind {
    Add,
    Delete,
    Update,
};

struct UpdateChunk {
    bool has_change_context;
    std::string change_context;
    std::vector<std::string> old_lines;
    std::vector<std::string> new_lines;
    bool is_end_of_file;
};

struct PatchAction {
    PatchKind kind;
    std::string path;
    std::string move_to;
    std::vector<std::string> lines;
    std::vector<UpdateChunk> chunks;
};

enum class LineEndingKind {
    Lf,
    Crlf,
};

enum class NormalizedPathKind {
    Relative,
    Absolute,
};

struct NormalizedPathPrefix {
    std::string value;
    std::size_t start;
};

std::string unique_atomic_write_temp_path(const std::string& path) {
    static std::atomic<unsigned long> next_suffix(1UL);

    std::ostringstream out;
    out << path << ".tmp."
#ifdef _WIN32
        << static_cast<unsigned long>(GetCurrentProcessId())
#else
        << static_cast<long>(getpid())
#endif
        << "." << platform::monotonic_ms() << "." << next_suffix.fetch_add(1UL);
    return out.str();
}

NormalizedPathPrefix normalized_path_prefix(const std::string& raw, NormalizedPathKind kind) {
    NormalizedPathPrefix prefix;
    prefix.start = 0;

    if (kind == NormalizedPathKind::Relative) {
        return prefix;
    }

#ifdef _WIN32
    if (raw.size() >= 3 && std::isalpha(static_cast<unsigned char>(raw[0])) != 0 && raw[1] == ':' &&
        (raw[2] == '\\' || raw[2] == '/')) {
        prefix.value = raw.substr(0, 2);
        prefix.value.push_back(path_utils::native_separator());
        prefix.start = 3;
        return prefix;
    }
    if (raw.rfind("\\\\", 0) == 0 || raw.rfind("//", 0) == 0) {
        prefix.value = "\\\\";
        prefix.start = 2;
        return prefix;
    }
#else
    if (!raw.empty() && raw[0] == '/') {
        prefix.value = "/";
        prefix.start = 1;
        return prefix;
    }
#endif

    throw std::runtime_error("absolute patch path is not supported");
}

void push_normalized_segment(std::vector<std::string>* parts, const std::string& segment, NormalizedPathKind kind) {
    if (segment.empty() || segment == ".") {
        return;
    }
    if (segment == "..") {
        if (parts->empty()) {
            if (kind == NormalizedPathKind::Relative) {
                throw std::runtime_error("path traversal is not supported");
            }
        } else {
            parts->pop_back();
        }
        return;
    }
    parts->push_back(segment);
}

std::string build_normalized_path(const std::vector<std::string>& parts, const std::string& prefix) {
    std::ostringstream out;
    out << prefix;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        if (i != 0 || (!prefix.empty() && prefix[prefix.size() - 1] != path_utils::native_separator())) {
            out << path_utils::native_separator();
        }
        out << parts[i];
    }
    return out.str();
}

std::string normalize_path_segments(const std::string& raw, NormalizedPathKind kind) {
    if (raw.empty()) {
        throw std::runtime_error("patch path is empty");
    }

    const NormalizedPathPrefix prefix = normalized_path_prefix(raw, kind);

    std::vector<std::string> parts;
    std::string current;
    for (std::size_t i = prefix.start; i < raw.size(); ++i) {
        const char ch = raw[i];
        if (ch == '/' || ch == '\\') {
            push_normalized_segment(&parts, current, kind);
            current.clear();
        } else {
            current.push_back(ch);
        }
    }
    push_normalized_segment(&parts, current, kind);

    if (parts.empty()) {
        throw std::runtime_error("patch path is empty");
    }

    return build_normalized_path(parts, prefix.value);
}

std::string normalize_relative_path(const std::string& raw) {
    return normalize_path_segments(raw, NormalizedPathKind::Relative);
}

std::string normalize_absolute_path(const std::string& raw) {
    return normalize_path_segments(raw, NormalizedPathKind::Absolute);
}

std::string normalize_patch_path(const std::string& raw) {
    const PathPolicy policy = host_path_policy();
    const std::string normalized = normalize_for_system(policy, raw);
    if (is_absolute_for_policy(policy, raw) || is_absolute_for_policy(policy, normalized)) {
        return normalize_absolute_path(normalized);
    }
    if (raw.size() >= 2 && raw[1] == ':') {
        throw std::runtime_error("drive-relative patch paths are not supported");
    }
    return normalize_relative_path(normalized);
}

std::string resolve_patch_path(const std::string& root, const std::string& path) {
    if (is_absolute_for_policy(host_path_policy(), path)) {
        return path;
    }
    return path_utils::join_path(root, path);
}

bool file_exists(const std::string& path) {
    struct stat st;
    return path_utils::stat_path(path, &st);
}

std::string read_text_file(const std::string& path) {
    ScopedFile input(path_utils::open_file(path, "rb"));
    if (!input.valid()) {
        throw std::runtime_error("unable to read " + path);
    }
    std::string text;
    char buffer[8192];
    while (true) {
        const std::size_t received = std::fread(buffer, 1, sizeof(buffer), input.get());
        if (received > 0U) {
            text.append(buffer, received);
        }
        if (received < sizeof(buffer)) {
            if (std::ferror(input.get()) != 0) {
                throw std::runtime_error("unable to read " + path);
            }
            break;
        }
    }
    return text;
}

void write_text_atomic(const std::string& path, const std::string& content) {
    path_utils::create_parent_directories(path);
    const std::string temp_path = unique_atomic_write_temp_path(path);
#ifndef _WIN32
    struct stat existing;
    const bool preserve_mode = path_utils::stat_path(path, &existing);
#endif

    ScopedFile output(path_utils::open_file(temp_path, "wb"));
    if (!output.valid()) {
        throw std::runtime_error("unable to write " + temp_path);
    }
    if (!content.empty() && std::fwrite(content.data(), 1, content.size(), output.get()) != content.size()) {
        throw std::runtime_error("unable to write " + temp_path);
    }
    if (output.close() != 0) {
        throw std::runtime_error("unable to write " + temp_path);
    }
#ifndef _WIN32
    if (preserve_mode && chmod(temp_path.c_str(), existing.st_mode) != 0) {
        (void)path_utils::remove_path(temp_path);
        throw std::runtime_error("unable to preserve mode for " + temp_path);
    }
#endif

    if (!path_utils::rename_path(temp_path, path)) {
        (void)path_utils::remove_path(temp_path);
        throw std::runtime_error("unable to rename " + temp_path + " to " + path);
    }
}

void remove_file_required(const std::string& path) {
    if (!path_utils::remove_path(path)) {
        throw std::runtime_error("unable to remove " + path);
    }
}

std::vector<std::string> split_lines(const std::string& text, bool* trailing_newline) {
    *trailing_newline = !text.empty() && text[text.size() - 1] == '\n';
    std::vector<std::string> lines;
    std::string current;

    for (std::size_t i = 0; i < text.size(); ++i) {
        const char ch = text[i];
        if (ch == '\n') {
            if (!current.empty() && current[current.size() - 1] == '\r') {
                current.erase(current.size() - 1);
            }
            lines.push_back(current);
            current.clear();
        } else {
            current.push_back(ch);
        }
    }

    if (!current.empty()) {
        if (!current.empty() && current[current.size() - 1] == '\r') {
            current.erase(current.size() - 1);
        }
        lines.push_back(current);
    }

    return lines;
}

LineEndingKind detect_line_ending(const std::string& text) {
    for (std::size_t i = 0; i < text.size(); ++i) {
        if (text[i] != '\n') {
            continue;
        }
        if (i > 0 && text[i - 1] == '\r') {
            return LineEndingKind::Crlf;
        }
        return LineEndingKind::Lf;
    }
    return LineEndingKind::Lf;
}

const char* line_ending_text(LineEndingKind line_ending) {
    return line_ending == LineEndingKind::Crlf ? "\r\n" : "\n";
}

std::string join_lines(const std::vector<std::string>& lines, bool trailing_newline, LineEndingKind line_ending) {
    std::ostringstream out;
    for (std::size_t i = 0; i < lines.size(); ++i) {
        if (i != 0) {
            out << line_ending_text(line_ending);
        }
        out << lines[i];
    }
    if (trailing_newline && !lines.empty()) {
        out << line_ending_text(line_ending);
    }
    return out.str();
}

static bool starts_with(const std::string& line, const char* prefix) {
    return line.rfind(prefix, 0) == 0;
}

static bool is_structural_line(const std::string& line) {
    return starts_with(line, "*** ");
}

static bool is_update_data_line(const std::string& line) {
    return !line.empty() && (line[0] == ' ' || line[0] == '+' || line[0] == '-');
}

std::vector<std::string> split_patch_lines(const std::string& patch_text) {
    std::istringstream input(patch_text);
    std::vector<std::string> lines;
    std::string line;
    while (std::getline(input, line)) {
        if (!line.empty() && line[line.size() - 1] == '\r') {
            line.erase(line.size() - 1);
        }
        lines.push_back(line);
    }
    return lines;
}

static void parse_update_chunk_line(const std::string& line, UpdateChunk* chunk) {
    const std::string value = line.substr(1);
    if (line[0] == ' ') {
        chunk->old_lines.push_back(value);
        chunk->new_lines.push_back(value);
        return;
    }
    if (line[0] == '-') {
        chunk->old_lines.push_back(value);
        return;
    }
    if (line[0] == '+') {
        chunk->new_lines.push_back(value);
        return;
    }
    throw std::runtime_error("invalid update hunk line");
}

std::vector<PatchAction> parse_patch(const std::string& patch_text) {
    const std::vector<std::string> lines = split_patch_lines(patch_text);
    if (lines.empty() || lines.front() != "*** Begin Patch") {
        throw std::runtime_error("invalid patch header");
    }
    if (lines.size() < 2 || lines.back() != "*** End Patch") {
        throw std::runtime_error("invalid patch footer");
    }

    std::vector<PatchAction> actions;
    std::size_t index = 1;
    while (index + 1 < lines.size()) {
        const std::string& line = lines[index];
        if (starts_with(line, "*** Add File: ")) {
            PatchAction action;
            action.kind = PatchKind::Add;
            action.path = normalize_patch_path(line.substr(14));
            ++index;
            while (index + 1 < lines.size() && !is_structural_line(lines[index])) {
                if (lines[index].empty() || lines[index][0] != '+') {
                    throw std::runtime_error("add file lines must start with +");
                }
                action.lines.push_back(lines[index].substr(1));
                ++index;
            }
            actions.push_back(action);
            continue;
        }

        if (starts_with(line, "*** Delete File: ")) {
            PatchAction action;
            action.kind = PatchKind::Delete;
            action.path = normalize_patch_path(line.substr(17));
            actions.push_back(action);
            ++index;
            continue;
        }

        if (starts_with(line, "*** Update File: ")) {
            PatchAction action;
            action.kind = PatchKind::Update;
            action.path = normalize_patch_path(line.substr(17));
            ++index;

            if (index + 1 < lines.size() && starts_with(lines[index], "*** Move to: ")) {
                action.move_to = normalize_patch_path(lines[index].substr(13));
                ++index;
            }

            while (index + 1 < lines.size() && !is_structural_line(lines[index])) {
                UpdateChunk chunk;
                chunk.has_change_context = false;
                chunk.is_end_of_file = false;

                if (starts_with(lines[index], "@@")) {
                    if (lines[index] == "@@") {
                        chunk.has_change_context = false;
                    } else if (starts_with(lines[index], "@@ ")) {
                        chunk.has_change_context = true;
                        chunk.change_context = lines[index].substr(3);
                    } else {
                        throw std::runtime_error("invalid update hunk header");
                    }
                    ++index;
                } else if (!action.chunks.empty()) {
                    throw std::runtime_error("invalid update hunk header");
                }

                while (index + 1 < lines.size() && is_update_data_line(lines[index])) {
                    parse_update_chunk_line(lines[index], &chunk);
                    ++index;
                }
                if (index + 1 < lines.size() && lines[index] == "*** End of File") {
                    chunk.is_end_of_file = true;
                    ++index;
                }
                if (chunk.old_lines.empty() && chunk.new_lines.empty()) {
                    throw std::runtime_error("update hunk with no changes");
                }
                action.chunks.push_back(chunk);
            }

            if (action.chunks.empty()) {
                throw std::runtime_error("update file hunk is empty");
            }
            actions.push_back(action);
            continue;
        }

        throw std::runtime_error("unsupported patch line");
    }

    if (actions.empty()) {
        throw std::runtime_error("patch contained no actions");
    }
    return actions;
}

static std::size_t find_sequence(const std::vector<std::string>& lines,
                                 const std::vector<std::string>& needle,
                                 std::size_t start,
                                 bool require_eof) {
    if (needle.empty()) {
        return std::min(start, lines.size());
    }
    if (needle.size() > lines.size()) {
        return std::string::npos;
    }
    const std::size_t max_start = lines.size() - needle.size();
    for (std::size_t i = std::min(start, lines.size()); i <= max_start; ++i) {
        bool matches = true;
        for (std::size_t j = 0; j < needle.size(); ++j) {
            if (lines[i + j] != needle[j]) {
                matches = false;
                break;
            }
        }
        if (!matches) {
            continue;
        }
        if (require_eof && i + needle.size() != lines.size()) {
            continue;
        }
        return i;
    }
    return std::string::npos;
}

static std::size_t find_context_anchor(const std::vector<std::string>& lines,
                                       const std::string& context,
                                       std::size_t start,
                                       bool require_eof) {
    const std::vector<std::string> needle(1, context);
    return find_sequence(lines, needle, start, require_eof);
}

static void apply_update_chunk(std::vector<std::string>* lines, std::size_t* cursor, const UpdateChunk& chunk) {
    std::size_t search_start = std::min(*cursor, lines->size());
    if (chunk.has_change_context) {
        search_start = find_context_anchor(*lines, chunk.change_context, *cursor, false);
        if (search_start == std::string::npos) {
            throw std::runtime_error("patch context not found");
        }
    }

    if (!chunk.old_lines.empty()) {
        const std::size_t start = find_sequence(*lines, chunk.old_lines, search_start, chunk.is_end_of_file);
        if (start == std::string::npos) {
            throw std::runtime_error("patch removal not found");
        }
        lines->erase(lines->begin() + start, lines->begin() + start + chunk.old_lines.size());
        lines->insert(lines->begin() + start, chunk.new_lines.begin(), chunk.new_lines.end());
        *cursor = start + chunk.new_lines.size();
        return;
    }

    if (chunk.has_change_context) {
        const std::size_t anchor = search_start;
        const std::size_t insert_at = chunk.is_end_of_file ? lines->size() : anchor;
        lines->insert(lines->begin() + insert_at, chunk.new_lines.begin(), chunk.new_lines.end());
        *cursor = insert_at + chunk.new_lines.size() + (chunk.is_end_of_file ? 0 : 1);
        return;
    }

    const std::size_t insert_at = lines->size();
    lines->insert(lines->begin() + insert_at, chunk.new_lines.begin(), chunk.new_lines.end());
    *cursor = insert_at + chunk.new_lines.size();
}

static std::string apply_update_chunks(const std::string& old_text, const std::vector<UpdateChunk>& chunks) {
    bool had_trailing_newline = false;
    const LineEndingKind line_ending = detect_line_ending(old_text);
    std::vector<std::string> lines = split_lines(old_text, &had_trailing_newline);
    std::size_t cursor = 0;

    for (std::size_t i = 0; i < chunks.size(); ++i) {
        apply_update_chunk(&lines, &cursor, chunks[i]);
    }

    return join_lines(lines, had_trailing_newline || !lines.empty(), line_ending);
}

std::string render_added_content(const std::vector<std::string>& lines) {
    return join_lines(lines, !lines.empty(), LineEndingKind::Lf);
}

} // namespace

PatchApplyResult
apply_patch(const std::string& root, const std::string& patch_text, const PatchPathAuthorizer& authorizer) {
    const std::vector<PatchAction> actions = parse_patch(patch_text);
    std::vector<std::string> summary;

    for (std::size_t i = 0; i < actions.size(); ++i) {
        const PatchAction& action = actions[i];
        const std::string source_path = resolve_patch_path(root, action.path);
        const std::string destination_path =
            action.move_to.empty() ? source_path : resolve_patch_path(root, action.move_to);
        if (authorizer) {
            authorizer(source_path);
            if (destination_path != source_path) {
                authorizer(destination_path);
            }
        }

        if (action.kind == PatchKind::Add) {
            write_text_atomic(source_path, render_added_content(action.lines));
            summary.push_back("A " + action.path);
            continue;
        }

        if (action.kind == PatchKind::Delete) {
            remove_file_required(source_path);
            summary.push_back("D " + action.path);
            continue;
        }

        const std::string old_text = read_text_file(source_path);
        const std::string new_text = apply_update_chunks(old_text, action.chunks);
        write_text_atomic(destination_path, new_text);
        if (!action.move_to.empty() && destination_path != source_path && file_exists(source_path)) {
            remove_file_required(source_path);
        }
        summary.push_back("M " + (action.move_to.empty() ? action.path : action.move_to));
    }

    std::ostringstream out;
    out << "Success. Updated the following files:\n";
    for (std::size_t i = 0; i < summary.size(); ++i) {
        out << summary[i] << '\n';
    }
    return PatchApplyResult{out.str(), summary};
}
