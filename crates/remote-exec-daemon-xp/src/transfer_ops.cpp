#include <algorithm>
#include <cerrno>
#include <cctype>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <sstream>
#include <stdexcept>
#include <vector>

#ifdef _WIN32
#include <direct.h>
#include <windows.h>
#include <sys/stat.h>
#else
#include <dirent.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#endif

#include "transfer_ops.h"

namespace {

const std::size_t TAR_BLOCK_SIZE = 512;

struct DirectoryEntry {
    std::string name;
    bool is_directory;
    bool is_regular_file;
};

struct TarHeaderView {
    std::string path;
    char typeflag;
    std::uint64_t size;
};

bool is_absolute_path(const std::string& path) {
#ifdef _WIN32
    return (path.size() >= 3 && std::isalpha(static_cast<unsigned char>(path[0])) != 0 &&
            path[1] == ':' && (path[2] == '\\' || path[2] == '/')) ||
           path.rfind("\\\\", 0) == 0 || path.rfind("//", 0) == 0;
#else
    return !path.empty() && path[0] == '/';
#endif
}

std::string parent_directory(const std::string& path) {
    const std::size_t slash = path.find_last_of("/\\");
    if (slash == std::string::npos) {
        return "";
    }
    return path.substr(0, slash);
}

bool stat_path(const std::string& path, struct stat* st) {
    return stat(path.c_str(), st) == 0;
}

bool path_exists(const std::string& path) {
    struct stat st;
    return stat_path(path, &st);
}

bool is_regular_file(const std::string& path) {
    struct stat st;
    return stat_path(path, &st) && (st.st_mode & S_IFREG) != 0;
}

bool is_directory(const std::string& path) {
    struct stat st;
    return stat_path(path, &st) && (st.st_mode & S_IFDIR) != 0;
}

char os_separator() {
#ifdef _WIN32
    return '\\';
#else
    return '/';
#endif
}

std::string join_path(const std::string& base, const std::string& child) {
    if (base.empty()) {
        return child;
    }
    std::string joined = base;
    if (!joined.empty() && joined[joined.size() - 1] != '/' && joined[joined.size() - 1] != '\\') {
        joined.push_back(os_separator());
    }
    joined += child;
    return joined;
}

void make_directory_if_missing(const std::string& path) {
    if (path.empty() || is_directory(path)) {
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

void ensure_parent_directory(const std::string& path, bool create_parent) {
    const std::string parent = parent_directory(path);
    if (parent.empty()) {
        return;
    }
    if (!create_parent) {
        if (!is_directory(parent)) {
            throw std::runtime_error("destination parent does not exist");
        }
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

std::string read_binary_file(const std::string& path) {
    std::ifstream input(path.c_str(), std::ios::binary);
    if (!input) {
        throw std::runtime_error("transfer source missing");
    }
    return std::string((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
}

void write_binary_file(const std::string& path, const std::string& bytes) {
    std::ofstream output(path.c_str(), std::ios::binary | std::ios::trunc);
    if (!output) {
        throw std::runtime_error("unable to write destination file");
    }
    output.write(bytes.data(), static_cast<std::streamsize>(bytes.size()));
}

std::vector<DirectoryEntry> list_directory_entries(const std::string& path) {
    std::vector<DirectoryEntry> entries;
#ifdef _WIN32
    std::string pattern = path;
    if (!pattern.empty() && pattern[pattern.size() - 1] != '\\' && pattern[pattern.size() - 1] != '/') {
        pattern.push_back('\\');
    }
    pattern.push_back('*');

    WIN32_FIND_DATAA find_data;
    HANDLE handle = FindFirstFileA(pattern.c_str(), &find_data);
    if (handle == INVALID_HANDLE_VALUE) {
        throw std::runtime_error("unable to read directory " + path);
    }

    do {
        const std::string name(find_data.cFileName);
        if (name == "." || name == "..") {
            continue;
        }
        const bool entry_is_directory =
            (find_data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
        entries.push_back(DirectoryEntry{name, entry_is_directory, !entry_is_directory});
    } while (FindNextFileA(handle, &find_data) != 0);

    const DWORD last_error = GetLastError();
    FindClose(handle);
    if (last_error != ERROR_NO_MORE_FILES) {
        throw std::runtime_error("unable to read directory " + path);
    }
#else
    DIR* dir = opendir(path.c_str());
    if (dir == NULL) {
        throw std::runtime_error("unable to read directory " + path);
    }

    dirent* entry = NULL;
    while ((entry = readdir(dir)) != NULL) {
        const std::string name(entry->d_name);
        if (name == "." || name == "..") {
            continue;
        }
        const std::string child = join_path(path, name);
        struct stat st;
        if (!stat_path(child, &st)) {
            closedir(dir);
            throw std::runtime_error("unable to stat path " + child);
        }
        entries.push_back(
            DirectoryEntry{name, (st.st_mode & S_IFDIR) != 0, (st.st_mode & S_IFREG) != 0}
        );
    }
    closedir(dir);
#endif

    std::sort(
        entries.begin(),
        entries.end(),
        [](const DirectoryEntry& left, const DirectoryEntry& right) {
            return left.name < right.name;
        }
    );
    return entries;
}

void remove_existing_path(const std::string& path) {
    if (!path_exists(path)) {
        return;
    }

    if (is_directory(path)) {
        const std::vector<DirectoryEntry> entries = list_directory_entries(path);
        for (std::size_t i = 0; i < entries.size(); ++i) {
            remove_existing_path(join_path(path, entries[i].name));
        }
#ifdef _WIN32
        if (_rmdir(path.c_str()) != 0) {
#else
        if (rmdir(path.c_str()) != 0) {
#endif
            throw std::runtime_error("unable to remove existing directory " + path);
        }
        return;
    }

    if (std::remove(path.c_str()) != 0) {
        throw std::runtime_error("unable to remove existing file " + path);
    }
}

bool prepare_destination_path(
    const std::string& absolute_path,
    bool replace_existing,
    bool create_parent
) {
    const bool existed = path_exists(absolute_path);
    if (existed && !replace_existing) {
        throw std::runtime_error("destination path already exists");
    }

    ensure_parent_directory(absolute_path, create_parent);

    if (existed) {
        remove_existing_path(absolute_path);
    }

    return existed;
}

void write_string_field(std::string* header, std::size_t offset, std::size_t width, const std::string& value) {
    const std::size_t length = std::min(width, value.size());
    if (length > 0) {
        header->replace(offset, length, value.substr(0, length));
    }
}

void write_octal_field(std::string* header, std::size_t offset, std::size_t width, std::uint64_t value) {
    char buffer[64];
    std::snprintf(
        buffer,
        sizeof(buffer),
        "%0*llo",
        static_cast<int>(width - 1),
        static_cast<unsigned long long>(value)
    );
    const std::string digits(buffer);
    if (digits.size() > width - 1) {
        throw std::runtime_error("tar numeric field overflow");
    }

    std::string field(width, '\0');
    field.replace(width - 1 - digits.size(), digits.size(), digits);
    field[width - 1] = ' ';
    header->replace(offset, width, field);
}

void write_tar_checksum(std::string* header) {
    std::fill(header->begin() + 148, header->begin() + 156, ' ');
    unsigned int checksum = 0;
    for (std::size_t i = 0; i < header->size(); ++i) {
        checksum += static_cast<unsigned char>((*header)[i]);
    }
    write_octal_field(header, 148, 8, checksum);
}

std::string truncate_path_for_header(const std::string& path) {
    if (path.size() <= 100) {
        return path;
    }
    return path.substr(0, 100);
}

std::size_t tar_padding(std::size_t size) {
    const std::size_t remainder = size % TAR_BLOCK_SIZE;
    return remainder == 0 ? 0 : TAR_BLOCK_SIZE - remainder;
}

void append_padded_body(std::string* archive, const std::string& body) {
    archive->append(body);
    archive->append(tar_padding(body.size()), '\0');
}

void append_tar_header(
    std::string* archive,
    const std::string& path,
    char typeflag,
    std::uint64_t size,
    std::uint64_t mode
) {
    std::string header(TAR_BLOCK_SIZE, '\0');
    write_string_field(&header, 0, 100, truncate_path_for_header(path));
    write_octal_field(&header, 100, 8, mode);
    write_octal_field(&header, 108, 8, 0);
    write_octal_field(&header, 116, 8, 0);
    write_octal_field(&header, 124, 12, size);
    write_octal_field(&header, 136, 12, 0);
    header[156] = typeflag;
    write_string_field(&header, 257, 6, "ustar ");
    header[263] = ' ';
    header[264] = '\0';
    write_tar_checksum(&header);
    archive->append(header);
}

void append_gnu_long_name(std::string* archive, const std::string& path) {
    const std::string body = path + '\0';
    append_tar_header(archive, "././@LongLink", 'L', body.size(), 0644);
    append_padded_body(archive, body);
}

void append_directory_entry(std::string* archive, const std::string& rel_path) {
    if (rel_path.size() > 100) {
        append_gnu_long_name(archive, rel_path);
    }
    append_tar_header(archive, rel_path, '5', 0, 0755);
}

void append_file_entry(std::string* archive, const std::string& rel_path, const std::string& body) {
    if (rel_path.size() > 100) {
        append_gnu_long_name(archive, rel_path);
    }
    append_tar_header(archive, rel_path, '0', body.size(), 0644);
    append_padded_body(archive, body);
}

void append_directory_contents(
    std::string* archive,
    const std::string& current_path,
    const std::string& current_rel
) {
    const std::vector<DirectoryEntry> entries = list_directory_entries(current_path);
    for (std::size_t i = 0; i < entries.size(); ++i) {
        const DirectoryEntry& entry = entries[i];
        const std::string child_path = join_path(current_path, entry.name);
        const std::string child_rel =
            current_rel.empty() ? entry.name : current_rel + "/" + entry.name;

        if (entry.is_directory) {
            append_directory_entry(archive, child_rel);
            append_directory_contents(archive, child_path, child_rel);
            continue;
        }
        if (!entry.is_regular_file) {
            throw std::runtime_error("transfer source contains unsupported entry " + child_path);
        }

        append_file_entry(archive, child_rel, read_binary_file(child_path));
    }
}

std::string field_string(const char* data, std::size_t size) {
    std::size_t length = 0;
    while (length < size && data[length] != '\0') {
        ++length;
    }
    return std::string(data, length);
}

std::uint64_t parse_octal_field(const char* data, std::size_t size) {
    std::size_t index = 0;
    while (index < size && (data[index] == ' ' || data[index] == '\0')) {
        ++index;
    }
    std::uint64_t value = 0;
    while (index < size && data[index] >= '0' && data[index] <= '7') {
        value = (value * 8) + static_cast<std::uint64_t>(data[index] - '0');
        ++index;
    }
    return value;
}

bool is_zero_block(const char* block) {
    for (std::size_t i = 0; i < TAR_BLOCK_SIZE; ++i) {
        if (block[i] != '\0') {
            return false;
        }
    }
    return true;
}

bool checksum_valid(const char* block) {
    const std::uint64_t stored = parse_octal_field(block + 148, 8);
    std::uint64_t computed = 0;
    for (std::size_t i = 0; i < TAR_BLOCK_SIZE; ++i) {
        if (i >= 148 && i < 156) {
            computed += static_cast<unsigned char>(' ');
        } else {
            computed += static_cast<unsigned char>(block[i]);
        }
    }
    return stored == computed;
}

std::string header_path(const char* block) {
    const std::string name = field_string(block, 100);
    const std::string prefix = field_string(block + 345, 155);
    if (prefix.empty()) {
        return name;
    }
    if (name.empty()) {
        return prefix;
    }
    return prefix + "/" + name;
}

TarHeaderView parse_header(const char* block) {
    if (!checksum_valid(block)) {
        throw std::runtime_error("invalid tar header checksum");
    }
    const char raw_type = block[156];
    return TarHeaderView{
        header_path(block),
        raw_type == '\0' ? '0' : raw_type,
        parse_octal_field(block + 124, 12),
    };
}

std::size_t padded_length(std::uint64_t size) {
    return static_cast<std::size_t>(size) + tar_padding(static_cast<std::size_t>(size));
}

std::string trim_trailing_slashes(std::string value) {
    while (value.size() > 1 && !value.empty() && value[value.size() - 1] == '/') {
        value.erase(value.size() - 1);
    }
    return value;
}

std::string read_gnu_long_name(
    const std::string& archive,
    std::size_t body_offset,
    std::uint64_t size
) {
    if (body_offset + padded_length(size) > archive.size()) {
        throw std::runtime_error("truncated tar entry body");
    }
    std::string value = archive.substr(body_offset, static_cast<std::size_t>(size));
    while (!value.empty() && value[value.size() - 1] == '\0') {
        value.erase(value.size() - 1);
    }
    return value;
}

std::vector<std::string> split_archive_path(const std::string& path) {
    std::vector<std::string> parts;
    std::string current;
    for (std::size_t i = 0; i < path.size(); ++i) {
        const char ch = path[i];
        if (ch == '/') {
            parts.push_back(current);
            current.clear();
            continue;
        }
        current.push_back(ch);
    }
    parts.push_back(current);
    return parts;
}

std::string normalize_archive_separators(std::string value) {
    std::replace(value.begin(), value.end(), '\\', '/');
    return value;
}

std::string validate_relative_archive_path(const std::string& raw_path) {
    std::string normalized = normalize_archive_separators(raw_path);
    while (normalized.rfind("./", 0) == 0) {
        normalized.erase(0, 2);
    }
    normalized = trim_trailing_slashes(normalized);

    if (normalized.empty() || normalized == ".") {
        return ".";
    }
    if (normalized[0] == '/') {
        throw std::runtime_error("archive path must be relative");
    }
    if (normalized.size() >= 2 &&
        std::isalpha(static_cast<unsigned char>(normalized[0])) != 0 &&
        normalized[1] == ':') {
        throw std::runtime_error("archive path must be relative");
    }
    if (normalized.rfind("//", 0) == 0) {
        throw std::runtime_error("archive path must be relative");
    }

    const std::vector<std::string> parts = split_archive_path(normalized);
    std::vector<std::string> cleaned;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        const std::string& part = parts[i];
        if (part.empty()) {
            throw std::runtime_error("archive path contains empty component");
        }
        if (part == "." || part == "..") {
            throw std::runtime_error("archive path escapes destination");
        }
        cleaned.push_back(part);
    }

    std::string result;
    for (std::size_t i = 0; i < cleaned.size(); ++i) {
        if (i != 0) {
            result.push_back('/');
        }
        result += cleaned[i];
    }
    return result;
}

std::string materialize_archive_path(
    const std::string& destination_root,
    const std::string& relative_archive_path
) {
    if (relative_archive_path == ".") {
        return destination_root;
    }

    const std::vector<std::string> parts = split_archive_path(relative_archive_path);
    std::string path = destination_root;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        path = join_path(path, parts[i]);
    }
    return path;
}

ExportedPayload export_directory_as_tar(const std::string& absolute_path) {
    std::string archive;
    append_directory_entry(&archive, ".");
    append_directory_contents(&archive, absolute_path, "");
    archive.append(TAR_BLOCK_SIZE * 2, '\0');
    return ExportedPayload{"directory", archive};
}

ImportSummary import_file_payload(
    const std::string& bytes,
    const std::string& absolute_path,
    bool replace_existing,
    bool create_parent
) {
    const bool existed = prepare_destination_path(absolute_path, replace_existing, create_parent);
    write_binary_file(absolute_path, bytes);

    return ImportSummary{
        "file",
        static_cast<std::uint64_t>(bytes.size()),
        1,
        0,
        existed,
    };
}

ImportSummary import_directory_from_tar(
    const std::string& archive,
    const std::string& absolute_path,
    bool replace_existing,
    bool create_parent
) {
    const bool replaced = prepare_destination_path(absolute_path, replace_existing, create_parent);
    make_directory_if_missing(absolute_path);

    ImportSummary summary = {"directory", 0, 0, 1, replaced};
    std::size_t offset = 0;
    std::string pending_long_name;

    while (offset < archive.size()) {
        if (archive.size() - offset < TAR_BLOCK_SIZE) {
            throw std::runtime_error("truncated tar header");
        }

        const char* block = archive.data() + offset;
        if (is_zero_block(block)) {
            break;
        }

        const TarHeaderView header = parse_header(block);
        offset += TAR_BLOCK_SIZE;

        if (offset + padded_length(header.size) > archive.size()) {
            throw std::runtime_error("truncated tar entry body");
        }

        if (header.typeflag == 'L') {
            pending_long_name = read_gnu_long_name(archive, offset, header.size);
            offset += padded_length(header.size);
            continue;
        }

        const std::string raw_path = pending_long_name.empty() ? header.path : pending_long_name;
        pending_long_name.clear();
        const std::string relative_path = validate_relative_archive_path(raw_path);
        const std::string output_path = materialize_archive_path(absolute_path, relative_path);

        if (header.typeflag == '5') {
            if (relative_path != ".") {
                ensure_parent_directory(output_path, true);
                make_directory_if_missing(output_path);
                summary.directories_copied += 1;
            }
            offset += padded_length(header.size);
            continue;
        }

        if (header.typeflag != '0') {
            throw std::runtime_error("archive contains unsupported entry");
        }
        if (relative_path == ".") {
            throw std::runtime_error("archive file entry cannot target root");
        }

        ensure_parent_directory(output_path, true);
        write_binary_file(
            output_path,
            archive.substr(offset, static_cast<std::size_t>(header.size))
        );
        summary.bytes_copied += header.size;
        summary.files_copied += 1;
        offset += padded_length(header.size);
    }

    if (!pending_long_name.empty()) {
        throw std::runtime_error("dangling GNU long name entry");
    }

    return summary;
}

} // namespace

ExportedPayload export_path(const std::string& absolute_path) {
    if (!is_absolute_path(absolute_path)) {
        throw std::runtime_error("transfer path is not absolute");
    }
    if (is_regular_file(absolute_path)) {
        return ExportedPayload{"file", read_binary_file(absolute_path)};
    }
    if (is_directory(absolute_path)) {
        return export_directory_as_tar(absolute_path);
    }
    throw std::runtime_error("transfer source must be a regular file or directory");
}

ImportSummary import_path(
    const std::string& bytes,
    const std::string& source_type,
    const std::string& absolute_path,
    bool replace_existing,
    bool create_parent
) {
    if (!is_absolute_path(absolute_path)) {
        throw std::runtime_error("transfer path is not absolute");
    }

    if (source_type == "file") {
        return import_file_payload(bytes, absolute_path, replace_existing, create_parent);
    }
    if (source_type == "directory") {
        return import_directory_from_tar(bytes, absolute_path, replace_existing, create_parent);
    }
    throw std::runtime_error("unsupported transfer source type");
}
