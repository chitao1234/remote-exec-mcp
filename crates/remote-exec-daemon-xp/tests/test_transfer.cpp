#include <algorithm>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>

#include "transfer_ops.h"

namespace fs = std::filesystem;

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

static void append_gnu_long_name(std::string* archive, const std::string& path) {
    append_tar_entry(archive, "././@LongLink", 'L', path + '\0');
}

static void append_tar_file(std::string& archive, const std::string& path, const std::string& body) {
    if (path.size() >= 100) {
        append_gnu_long_name(&archive, path);
    }
    append_tar_entry(&archive, path, '0', body);
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

static void assert_file_transfer() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-file";
    fs::remove_all(root);
    fs::create_directories(root);

    write_text(root / "source.txt", "hello transfer");
    const ExportedPayload exported = export_path((root / "source.txt").string());
    assert(exported.source_type == "file");
    assert(exported.bytes == "hello transfer");

    const ImportSummary imported =
        import_path(exported.bytes, exported.source_type, (root / "copied.txt").string(), true, true);
    assert(imported.files_copied == 1);
    assert(imported.directories_copied == 0);
    assert(read_text(root / "copied.txt") == "hello transfer");
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
        true,
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
        true,
        true
    );

    assert(imported.replaced);
    assert(!fs::exists(root / "dest" / "stale" / "old.txt"));
    assert(read_text(root / "dest" / "fresh.txt") == "fresh");
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
        true,
        true
    );

    assert(imported.source_type == "directory");
    assert(read_text(root / "dest" / long_name / "nested" / "payload.txt") == "long path");
}

static void assert_directory_traversal_is_rejected() {
    const std::string archive = tar_with_single_file("../escape.txt", "bad");
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-traversal";
    fs::remove_all(root);
    bool rejected = false;
    try {
        (void)import_path(archive, "directory", (root / "dest").string(), true, true);
    } catch (...) {
        rejected = true;
    }
    assert(rejected);
}

int main() {
    assert_file_transfer();
    assert_directory_round_trip();
    assert_directory_replace_behavior();
    assert_directory_long_path_round_trip();
    assert_directory_traversal_is_rejected();
    return 0;
}
