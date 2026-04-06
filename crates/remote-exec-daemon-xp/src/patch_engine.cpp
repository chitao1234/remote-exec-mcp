#include <algorithm>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <sstream>
#include <stdexcept>
#include <vector>

#ifdef _WIN32
#include <direct.h>
#include <sys/stat.h>
#else
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#endif

#include "patch_engine.h"

namespace {

enum PatchKind {
    PATCH_ADD,
    PATCH_DELETE,
    PATCH_UPDATE,
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

char native_separator() {
#ifdef _WIN32
    return '\\';
#else
    return '/';
#endif
}

std::string normalize_relative_path(const std::string& raw) {
    if (raw.empty()) {
        throw std::runtime_error("patch path is empty");
    }
    if (raw[0] == '/' || raw[0] == '\\') {
        throw std::runtime_error("absolute patch paths are not supported");
    }
    if (raw.size() >= 2 && raw[1] == ':') {
        throw std::runtime_error("absolute patch paths are not supported");
    }

    std::vector<std::string> parts;
    std::string current;
    for (std::size_t i = 0; i < raw.size(); ++i) {
        const char ch = raw[i];
        if (ch == '/' || ch == '\\') {
            if (!current.empty()) {
                if (current == "..") {
                    throw std::runtime_error("path traversal is not supported");
                }
                if (current != ".") {
                    parts.push_back(current);
                }
                current.clear();
            }
        } else {
            current.push_back(ch);
        }
    }
    if (!current.empty()) {
        if (current == "..") {
            throw std::runtime_error("path traversal is not supported");
        }
        if (current != ".") {
            parts.push_back(current);
        }
    }
    if (parts.empty()) {
        throw std::runtime_error("patch path is empty");
    }

    std::ostringstream out;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        if (i != 0) {
            out << native_separator();
        }
        out << parts[i];
    }
    return out.str();
}

std::string join_path(const std::string& base, const std::string& relative) {
    if (base.empty()) {
        return relative;
    }
    std::string joined = base;
    const char sep = native_separator();
    if (joined[joined.size() - 1] != '/' && joined[joined.size() - 1] != '\\') {
        joined.push_back(sep);
    }
    joined += relative;
    return joined;
}

std::string parent_directory(const std::string& path) {
    const std::size_t slash = path.find_last_of("/\\");
    if (slash == std::string::npos) {
        return "";
    }
    return path.substr(0, slash);
}

void make_directory_if_missing(const std::string& path) {
    if (path.empty()) {
        return;
    }
#ifdef _WIN32
    if (_mkdir(path.c_str()) != 0 && errno != EEXIST) {
#else
    if (mkdir(path.c_str(), 0777) != 0 && errno != EEXIST) {
#endif
        throw std::runtime_error("unable to create directory " + path);
    }
}

void create_parent_directories(const std::string& path) {
    const std::string parent = parent_directory(path);
    if (parent.empty()) {
        return;
    }

    std::string current;
    for (std::size_t i = 0; i < parent.size(); ++i) {
        const char ch = parent[i];
        current.push_back(ch);
        if (ch != '/' && ch != '\\') {
            continue;
        }
        if (current.size() == 1) {
            continue;
        }
        if (current.size() == 3 && current[1] == ':') {
            continue;
        }
        current.erase(current.size() - 1);
        make_directory_if_missing(current);
        current.push_back(ch);
    }
    make_directory_if_missing(parent);
}

bool file_exists(const std::string& path) {
    struct stat st;
    return stat(path.c_str(), &st) == 0;
}

std::string read_text_file(const std::string& path) {
    std::ifstream input(path.c_str(), std::ios::binary);
    if (!input) {
        throw std::runtime_error("unable to read " + path);
    }
    return std::string((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
}

void write_text_atomic(const std::string& path, const std::string& content) {
    create_parent_directories(path);
    const std::string temp_path = path + ".tmp";

    std::ofstream output(temp_path.c_str(), std::ios::binary | std::ios::trunc);
    if (!output) {
        throw std::runtime_error("unable to write " + temp_path);
    }
    output << content;
    output.close();

    std::remove(path.c_str());
    if (std::rename(temp_path.c_str(), path.c_str()) != 0) {
        throw std::runtime_error("unable to rename " + temp_path + " to " + path);
    }
}

void remove_file_required(const std::string& path) {
    if (std::remove(path.c_str()) != 0) {
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

std::string join_lines(const std::vector<std::string>& lines, bool trailing_newline) {
    std::ostringstream out;
    for (std::size_t i = 0; i < lines.size(); ++i) {
        if (i != 0) {
            out << '\n';
        }
        out << lines[i];
    }
    if (trailing_newline && !lines.empty()) {
        out << '\n';
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
            action.kind = PATCH_ADD;
            action.path = normalize_relative_path(line.substr(14));
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
            action.kind = PATCH_DELETE;
            action.path = normalize_relative_path(line.substr(17));
            actions.push_back(action);
            ++index;
            continue;
        }

        if (starts_with(line, "*** Update File: ")) {
            PatchAction action;
            action.kind = PATCH_UPDATE;
            action.path = normalize_relative_path(line.substr(17));
            ++index;

            if (index + 1 < lines.size() && starts_with(lines[index], "*** Move to: ")) {
                action.move_to = normalize_relative_path(lines[index].substr(13));
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

static std::size_t find_sequence(
    const std::vector<std::string>& lines,
    const std::vector<std::string>& needle,
    std::size_t start,
    bool require_eof
) {
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

static std::size_t find_context_anchor(
    const std::vector<std::string>& lines,
    const std::string& context,
    std::size_t start,
    bool require_eof
) {
    const std::vector<std::string> needle(1, context);
    return find_sequence(lines, needle, start, require_eof);
}

static void apply_update_chunk(
    std::vector<std::string>* lines,
    std::size_t* cursor,
    const UpdateChunk& chunk
) {
    std::size_t search_start = std::min(*cursor, lines->size());
    if (chunk.has_change_context) {
        search_start = find_context_anchor(*lines, chunk.change_context, *cursor, false);
        if (search_start == std::string::npos) {
            throw std::runtime_error("patch context not found");
        }
    }

    if (!chunk.old_lines.empty()) {
        const std::size_t start =
            find_sequence(*lines, chunk.old_lines, search_start, chunk.is_end_of_file);
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
        const std::size_t insert_at = chunk.is_end_of_file ? anchor + 1 : anchor;
        lines->insert(lines->begin() + insert_at, chunk.new_lines.begin(), chunk.new_lines.end());
        *cursor = insert_at + chunk.new_lines.size() + (chunk.is_end_of_file ? 0 : 1);
        return;
    }

    const std::size_t insert_at = chunk.is_end_of_file ? lines->size() : std::min(*cursor, lines->size());
    lines->insert(lines->begin() + insert_at, chunk.new_lines.begin(), chunk.new_lines.end());
    *cursor = insert_at + chunk.new_lines.size();
}

static std::string apply_update_chunks(
    const std::string& old_text,
    const std::vector<UpdateChunk>& chunks
) {
    bool had_trailing_newline = false;
    std::vector<std::string> lines = split_lines(old_text, &had_trailing_newline);
    std::size_t cursor = 0;

    for (std::size_t i = 0; i < chunks.size(); ++i) {
        apply_update_chunk(&lines, &cursor, chunks[i]);
    }

    return join_lines(lines, had_trailing_newline || !lines.empty());
}

std::string render_added_content(const std::vector<std::string>& lines) {
    return join_lines(lines, !lines.empty());
}

} // namespace

PatchApplyResult apply_patch(const std::string& root, const std::string& patch_text) {
    const std::vector<PatchAction> actions = parse_patch(patch_text);
    std::vector<std::string> summary;

    for (std::size_t i = 0; i < actions.size(); ++i) {
        const PatchAction& action = actions[i];
        const std::string source_path = join_path(root, action.path);
        const std::string destination_path =
            action.move_to.empty() ? source_path : join_path(root, action.move_to);

        if (action.kind == PATCH_ADD) {
            write_text_atomic(source_path, render_added_content(action.lines));
            summary.push_back("A " + action.path);
            continue;
        }

        if (action.kind == PATCH_DELETE) {
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
    return PatchApplyResult{out.str()};
}
