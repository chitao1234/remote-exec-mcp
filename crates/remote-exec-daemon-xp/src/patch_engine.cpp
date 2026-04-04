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

struct PatchAction {
    PatchKind kind;
    std::string path;
    std::string move_to;
    std::vector<std::string> body;
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

std::vector<PatchAction> parse_patch(const std::string& patch_text) {
    std::istringstream input(patch_text);
    std::string line;
    std::vector<PatchAction> actions;
    PatchAction* current = NULL;
    bool saw_begin = false;

    while (std::getline(input, line)) {
        if (!line.empty() && line[line.size() - 1] == '\r') {
            line.erase(line.size() - 1);
        }

        if (line == "*** Begin Patch") {
            saw_begin = true;
            continue;
        }
        if (line == "*** End Patch") {
            break;
        }
        if (line == "*** End of File") {
            continue;
        }
        if (!saw_begin) {
            throw std::runtime_error("invalid patch header");
        }

        if (line.rfind("*** Add File: ", 0) == 0) {
            actions.push_back(PatchAction{PATCH_ADD, normalize_relative_path(line.substr(14)), "", {}});
            current = &actions.back();
            continue;
        }
        if (line.rfind("*** Delete File: ", 0) == 0) {
            actions.push_back(PatchAction{PATCH_DELETE, normalize_relative_path(line.substr(17)), "", {}});
            current = &actions.back();
            continue;
        }
        if (line.rfind("*** Update File: ", 0) == 0) {
            actions.push_back(PatchAction{PATCH_UPDATE, normalize_relative_path(line.substr(17)), "", {}});
            current = &actions.back();
            continue;
        }
        if (line.rfind("*** Move to: ", 0) == 0) {
            if (current == NULL) {
                throw std::runtime_error("move target without active file");
            }
            current->move_to = normalize_relative_path(line.substr(13));
            continue;
        }
        if (line.rfind("@@", 0) == 0 ||
            (!line.empty() && (line[0] == '+' || line[0] == '-' || line[0] == ' '))) {
            if (current == NULL) {
                throw std::runtime_error("patch body without active file");
            }
            current->body.push_back(line);
        }
    }

    if (!saw_begin || actions.empty()) {
        throw std::runtime_error("patch contained no actions");
    }

    return actions;
}

std::string apply_update_body(const std::string& old_text, const std::vector<std::string>& body) {
    bool had_trailing_newline = false;
    const std::vector<std::string> old_lines = split_lines(old_text, &had_trailing_newline);
    std::vector<std::string> new_lines;
    std::size_t cursor = 0;
    bool touched = false;

    for (std::size_t i = 0; i < body.size(); ++i) {
        const std::string& line = body[i];
        if (line.rfind("@@", 0) == 0) {
            continue;
        }
        if (line.empty()) {
            continue;
        }

        const char prefix = line[0];
        const std::string text = line.substr(1);

        if (prefix == ' ') {
            if (cursor >= old_lines.size() || old_lines[cursor] != text) {
                throw std::runtime_error("patch context not found");
            }
            new_lines.push_back(old_lines[cursor]);
            ++cursor;
        } else if (prefix == '-') {
            if (cursor >= old_lines.size() || old_lines[cursor] != text) {
                throw std::runtime_error("patch removal not found");
            }
            ++cursor;
            touched = true;
        } else if (prefix == '+') {
            new_lines.push_back(text);
            touched = true;
        }
    }

    if (!touched) {
        return old_text;
    }

    while (cursor < old_lines.size()) {
        new_lines.push_back(old_lines[cursor]);
        ++cursor;
    }

    return join_lines(new_lines, had_trailing_newline || !new_lines.empty());
}

std::string render_added_content(const std::vector<std::string>& body) {
    std::vector<std::string> lines;
    for (std::size_t i = 0; i < body.size(); ++i) {
        const std::string& line = body[i];
        if (!line.empty() && line[0] == '+') {
            lines.push_back(line.substr(1));
        }
    }
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
            write_text_atomic(source_path, render_added_content(action.body));
            summary.push_back("A " + action.path);
            continue;
        }

        if (action.kind == PATCH_DELETE) {
            remove_file_required(source_path);
            summary.push_back("D " + action.path);
            continue;
        }

        const std::string old_text = read_text_file(source_path);
        const std::string new_text =
            action.body.empty() ? old_text : apply_update_body(old_text, action.body);
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
