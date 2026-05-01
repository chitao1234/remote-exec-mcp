#include <algorithm>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <string>
#include <utility>
#include <vector>
#ifndef _WIN32
#include <sys/stat.h>
#endif

#include "transfer_ops.h"

namespace fs = std::filesystem;

static const char* const SINGLE_FILE_ENTRY = ".remote-exec-file";
static const char* const TRANSFER_SUMMARY_ENTRY = ".remote-exec-transfer-summary.json";

static std::string read_text(const fs::path& path) {
    std::ifstream input(path.c_str(), std::ios::binary);
    return std::string((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
}

static void write_text(const fs::path& path, const std::string& value) {
    std::ofstream output(path.c_str(), std::ios::binary | std::ios::trunc);
    output << value;
}

static std::string octal_field(std::size_t width, std::uint64_t value) {
    char buffer[64];
    std::snprintf(
        buffer,
        sizeof(buffer),
        "%0*llo",
        static_cast<int>(width - 1),
        static_cast<unsigned long long>(value)
    );
    std::string field(width, '\0');
    const std::string digits(buffer);
    const std::size_t start = width - 1 - std::min(width - 1, digits.size());
    field.replace(start, std::min(width - 1, digits.size()), digits.substr(digits.size() - std::min(width - 1, digits.size())));
    field[width - 1] = ' ';
    return field;
}

static void set_bytes(std::string* header, std::size_t offset, std::size_t width, const std::string& value) {
    header->replace(offset, std::min(width, value.size()), value.substr(0, width));
}

static void write_checksum(std::string* header) {
    std::fill(header->begin() + 148, header->begin() + 156, ' ');
    unsigned int checksum = 0;
    for (unsigned char ch : *header) {
        checksum += ch;
    }
    const std::string field = octal_field(8, checksum);
    header->replace(148, 8, field);
}

static void append_padded_bytes(std::string* archive, const std::string& bytes) {
    archive->append(bytes);
    const std::size_t remainder = bytes.size() % 512;
    if (remainder != 0) {
        archive->append(512 - remainder, '\0');
    }
}

static void append_tar_entry(
    std::string* archive,
    const std::string& path,
    char typeflag,
    const std::string& body
) {
    std::string header(512, '\0');
    set_bytes(&header, 0, 100, path);
    header.replace(100, 8, octal_field(8, typeflag == '5' ? 0755 : 0644));
    header.replace(108, 8, octal_field(8, 0));
    header.replace(116, 8, octal_field(8, 0));
    header.replace(124, 12, octal_field(12, body.size()));
    header.replace(136, 12, octal_field(12, 0));
    header[156] = typeflag;
    set_bytes(&header, 257, 6, "ustar ");
    set_bytes(&header, 263, 2, " \0");
    write_checksum(&header);

    archive->append(header);
    append_padded_bytes(archive, body);
}

static void append_tar_symlink(
    std::string* archive,
    const std::string& path,
    const std::string& target
) {
    std::string header(512, '\0');
    set_bytes(&header, 0, 100, path);
    header.replace(100, 8, octal_field(8, 0777));
    header.replace(108, 8, octal_field(8, 0));
    header.replace(116, 8, octal_field(8, 0));
    header.replace(124, 12, octal_field(12, 0));
    header.replace(136, 12, octal_field(12, 0));
    header[156] = '2';
    set_bytes(&header, 157, 100, target);
    set_bytes(&header, 257, 6, "ustar ");
    set_bytes(&header, 263, 2, " \0");
    write_checksum(&header);

    archive->append(header);
}

static void append_gnu_long_name(std::string* archive, const std::string& path) {
    append_tar_entry(archive, "././@LongLink", 'L', path + '\0');
}

static void append_tar_file(std::string& archive, const std::string& path, const std::string& body) {
    if (path.size() >= 100) {
        append_gnu_long_name(&archive, path);
    }
    append_tar_entry(&archive, path, '0', body);
}

static void append_tar_directory(std::string& archive, const std::string& path) {
    if (path.size() >= 100) {
        append_gnu_long_name(&archive, path);
    }
    append_tar_entry(&archive, path, '5', "");
}

static void finalize_tar(std::string& archive) {
    archive.append(1024, '\0');
}

static std::string tar_with_single_file(const std::string& path, const std::string& body) {
    std::string archive;
    append_tar_file(archive, path, body);
    finalize_tar(archive);
    return archive;
}

static std::uint64_t parse_octal_value(const char* data, std::size_t size) {
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

static bool block_is_zero(const char* block) {
    for (std::size_t i = 0; i < 512; ++i) {
        if (block[i] != '\0') {
            return false;
        }
    }
    return true;
}

static std::pair<std::string, std::string> read_single_file_tar(const std::string& archive) {
    assert(archive.size() >= 512);

    const char* header = archive.data();
    std::size_t path_length = 0;
    while (path_length < 100 && header[path_length] != '\0') {
        ++path_length;
    }
    const std::string path(header, path_length);
    const char typeflag = header[156] == '\0' ? '0' : header[156];
    assert(typeflag == '0');

    const std::uint64_t size = parse_octal_value(header + 124, 12);
    const std::size_t body_offset = 512;
    const std::size_t body_size = static_cast<std::size_t>(size);
    const std::size_t padded_size = ((body_size + 511) / 512) * 512;
    assert(body_offset + padded_size <= archive.size());

    for (std::size_t offset = body_offset + padded_size; offset < archive.size(); offset += 512) {
        assert(offset + 512 <= archive.size());
        assert(block_is_zero(archive.data() + offset));
    }

    return std::make_pair(path, archive.substr(body_offset, body_size));
}

static void assert_file_transfer() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-file";
    fs::remove_all(root);
    fs::create_directories(root);

    write_text(root / "source.txt", "hello transfer");
    const ExportedPayload exported = export_path((root / "source.txt").string());
    assert(exported.source_type == "file");
    const std::pair<std::string, std::string> file_entry = read_single_file_tar(exported.bytes);
    assert(file_entry.first == SINGLE_FILE_ENTRY);
    assert(file_entry.second == "hello transfer");

    const ImportSummary imported = import_path(
        exported.bytes,
        exported.source_type,
        (root / "copied.txt").string(),
        "replace",
        true
    );
    assert(imported.files_copied == 1);
    assert(imported.directories_copied == 0);
    assert(read_text(root / "copied.txt") == "hello transfer");
}

static void assert_file_transfer_rejects_unexpected_entry_path() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-file-reject";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)import_path(
            tar_with_single_file("payload.txt", "bad"),
            "file",
            (root / "copied.txt").string(),
            "replace",
            true
        );
    } catch (...) {
        rejected = true;
    }

    assert(rejected);
}

static void assert_file_transfer_rejects_raw_bytes() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-file-raw";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)import_path(
            "raw-bytes",
            "file",
            (root / "copied.txt").string(),
            "replace",
            true
        );
    } catch (...) {
        rejected = true;
    }

    assert(rejected);
}

static void assert_directory_round_trip() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-dir";
    fs::remove_all(root);
    fs::create_directories(root / "source" / "nested" / "empty");
    write_text(root / "source" / "nested" / "hello.txt", "hello directory");
    write_text(root / "source" / "top.txt", "top level");

    const ExportedPayload exported = export_path((root / "source").string());
    assert(exported.source_type == "directory");

    const ImportSummary imported = import_path(
        exported.bytes,
        exported.source_type,
        (root / "dest").string(),
        "replace",
        true
    );

    assert(imported.source_type == "directory");
    assert(imported.files_copied == 2);
    assert(imported.directories_copied >= 3);
    assert(read_text(root / "dest" / "nested" / "hello.txt") == "hello directory");
    assert(read_text(root / "dest" / "top.txt") == "top level");
    assert(fs::is_directory(root / "dest" / "nested" / "empty"));
}

static void assert_directory_replace_behavior() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-replace";
    fs::remove_all(root);
    fs::create_directories(root / "source");
    fs::create_directories(root / "dest" / "stale");
    write_text(root / "source" / "fresh.txt", "fresh");
    write_text(root / "dest" / "stale" / "old.txt", "old");

    const ExportedPayload exported = export_path((root / "source").string());
    const ImportSummary imported = import_path(
        exported.bytes,
        exported.source_type,
        (root / "dest").string(),
        "replace",
        true
    );

    assert(imported.replaced);
    assert(!fs::exists(root / "dest" / "stale" / "old.txt"));
    assert(read_text(root / "dest" / "fresh.txt") == "fresh");
}

static void assert_path_info_reports_existing_directory() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-path-info";
    fs::remove_all(root);
    fs::create_directories(root / "dest");

    const PathInfo existing = path_info((root / "dest").string());
    assert(existing.exists);
    assert(existing.is_directory);

    const PathInfo missing = path_info((root / "missing").string());
    assert(!missing.exists);
    assert(!missing.is_directory);
}

static void assert_directory_merge_behavior() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-merge";
    fs::remove_all(root);
    fs::create_directories(root / "source" / "nested");
    fs::create_directories(root / "dest" / "nested");
    write_text(root / "source" / "nested" / "fresh.txt", "fresh");
    write_text(root / "dest" / "stale.txt", "stale");
    write_text(root / "dest" / "nested" / "old.txt", "old");

    const ExportedPayload exported = export_path((root / "source").string());
    const ImportSummary imported = import_path(
        exported.bytes,
        exported.source_type,
        (root / "dest").string(),
        "merge",
        true
    );

    assert(!imported.replaced);
    assert(read_text(root / "dest" / "nested" / "fresh.txt") == "fresh");
    assert(read_text(root / "dest" / "stale.txt") == "stale");
    assert(read_text(root / "dest" / "nested" / "old.txt") == "old");
}

static void assert_directory_long_path_round_trip() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-long";
    fs::remove_all(root);

    const std::string long_name =
        "very-long-segment-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const fs::path long_file = root / "source" / long_name / "nested" / "payload.txt";
    fs::create_directories(long_file.parent_path());
    write_text(long_file, "long path");

    const ExportedPayload exported = export_path((root / "source").string());
    const ImportSummary imported = import_path(
        exported.bytes,
        exported.source_type,
        (root / "dest").string(),
        "replace",
        true
    );

    assert(imported.source_type == "directory");
    assert(read_text(root / "dest" / long_name / "nested" / "payload.txt") == "long path");
}

#ifndef _WIN32
static void assert_symlink_sources_are_rejected() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink";
    fs::remove_all(root);
    fs::create_directories(root / "source");
    write_text(root / "target.txt", "target");
    write_text(root / "source" / "regular.txt", "regular");
    fs::create_symlink(root / "target.txt", root / "link.txt");
    fs::create_symlink(root / "target.txt", root / "source" / "link.txt");

    bool rejected = false;
    try {
        (void)export_path((root / "link.txt").string(), "reject");
    } catch (...) {
        rejected = true;
    }
    assert(rejected);

    rejected = false;
    try {
        (void)export_path((root / "source").string(), "reject");
    } catch (...) {
        rejected = true;
    }
    assert(rejected);
}

static void assert_symlink_sources_are_preserved_by_default() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-preserve";
    fs::remove_all(root);
    fs::create_directories(root / "source");
    write_text(root / "source" / "regular.txt", "regular");
    fs::create_symlink("regular.txt", root / "source" / "link.txt");

    const ExportedPayload exported = export_path((root / "source").string());
    assert(exported.source_type == "directory");
    assert(exported.bytes.find("link.txt") != std::string::npos);
}

static void assert_transfer_skips_special_files_with_warning() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-special-skip";
    fs::remove_all(root);
    fs::create_directories(root / "source");
    write_text(root / "source" / "regular.txt", "regular");
    const fs::path fifo = root / "source" / "events.fifo";
    assert(mkfifo(fifo.c_str(), 0600) == 0);

    const ExportedPayload exported = export_path((root / "source").string());
    assert(exported.source_type == "directory");
    assert(exported.bytes.find("regular.txt") != std::string::npos);
    assert(exported.bytes.find(TRANSFER_SUMMARY_ENTRY) != std::string::npos);

    const ImportSummary imported =
        import_path(exported.bytes, "directory", (root / "dest").string(), "replace", true);
    assert(imported.warnings.size() == 1);
    assert(imported.warnings[0].code == "transfer_skipped_unsupported_entry");
    assert(read_text(root / "dest" / "regular.txt") == "regular");
    assert(!fs::exists(root / "dest" / "events.fifo"));
    assert(!fs::exists(root / "dest" / TRANSFER_SUMMARY_ENTRY));
}

static void assert_symlink_import_preserves_links() {
    std::string archive;
    append_tar_file(archive, "alpha.txt", "alpha");
    append_tar_symlink(&archive, "alpha-link", "alpha.txt");
    finalize_tar(archive);

    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-import";
    fs::remove_all(root);

    const ImportSummary imported =
        import_path(archive, "directory", (root / "dest").string(), "replace", true);

    assert(imported.files_copied == 2);
    assert(read_text(root / "dest" / "alpha.txt") == "alpha");
    assert(fs::read_symlink(root / "dest" / "alpha-link") == fs::path("alpha.txt"));
}
#endif

static void assert_directory_traversal_is_rejected() {
    const std::string archive = tar_with_single_file("../escape.txt", "bad");
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-traversal";
    fs::remove_all(root);
    bool rejected = false;
    try {
        (void)import_path(archive, "directory", (root / "dest").string(), "replace", true);
    } catch (...) {
        rejected = true;
    }
    assert(rejected);
}

static void assert_multiple_sources_import() {
    std::string archive;
    append_tar_file(archive, "alpha.txt", "alpha");
    append_tar_directory(archive, "nested");
    append_tar_file(archive, "nested/beta.txt", "beta");
    finalize_tar(archive);

    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-multiple";
    fs::remove_all(root);

    const ImportSummary imported =
        import_path(archive, "multiple", (root / "dest").string(), "replace", true);

    assert(imported.source_type == "multiple");
    assert(imported.files_copied == 2);
    assert(imported.directories_copied >= 2);
    assert(read_text(root / "dest" / "alpha.txt") == "alpha");
    assert(read_text(root / "dest" / "nested" / "beta.txt") == "beta");
}

int main() {
    assert_file_transfer();
    assert_file_transfer_rejects_unexpected_entry_path();
    assert_file_transfer_rejects_raw_bytes();
    assert_directory_round_trip();
    assert_directory_replace_behavior();
    assert_path_info_reports_existing_directory();
    assert_directory_merge_behavior();
    assert_directory_long_path_round_trip();
#ifndef _WIN32
    assert_symlink_sources_are_rejected();
    assert_symlink_sources_are_preserved_by_default();
    assert_transfer_skips_special_files_with_warning();
    assert_symlink_import_preserves_links();
#endif
    assert_directory_traversal_is_rejected();
    assert_multiple_sources_import();
    return 0;
}
