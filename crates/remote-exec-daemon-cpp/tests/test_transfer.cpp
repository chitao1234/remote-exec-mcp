#include <algorithm>
#include "test_assert.h"
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <utility>
#include <vector>
#ifndef _WIN32
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>
#endif

#include "rpc_failures.h"
#include "test_contract_fixtures.h"
#include "test_filesystem.h"
#include "transfer_ops.h"

namespace fs = test_fs;

static const char* const SINGLE_FILE_ENTRY = ".remote-exec-file";
static const char* const TRANSFER_SUMMARY_ENTRY = ".remote-exec-transfer-summary.json";

static std::string read_text(const fs::path& path) {
    return fs::read_file_bytes(path);
}

static void write_text(const fs::path& path, const std::string& value) {
    fs::write_file_bytes(path, value);
}

static std::string octal_field(std::size_t width, std::uint64_t value) {
    char buffer[64];
    std::snprintf(
        buffer, sizeof(buffer), "%0*llo", static_cast<int>(width - 1), static_cast<unsigned long long>(value));
    std::string field(width, '\0');
    const std::string digits(buffer);
    const std::size_t start = width - 1 - std::min(width - 1, digits.size());
    field.replace(
        start, std::min(width - 1, digits.size()), digits.substr(digits.size() - std::min(width - 1, digits.size())));
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
    std::string* archive, const std::string& path, char typeflag, const std::string& body, std::uint64_t mode = 0644) {
    std::string header(512, '\0');
    set_bytes(&header, 0, 100, path);
    header.replace(100, 8, octal_field(8, typeflag == '5' ? 0755 : mode));
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

static void append_tar_symlink(std::string* archive, const std::string& path, const std::string& target) {
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

#ifndef _WIN32
static void
append_tar_file_with_mode(std::string& archive, const std::string& path, const std::string& body, std::uint64_t mode) {
    if (path.size() >= 100) {
        append_gnu_long_name(&archive, path);
    }
    append_tar_entry(&archive, path, '0', body, mode);
}
#endif

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

static std::string tar_with_declared_file_size(const std::string& path, std::uint64_t declared_size) {
    std::string header(512, '\0');
    set_bytes(&header, 0, 100, path);
    header.replace(100, 8, octal_field(8, 0644));
    header.replace(108, 8, octal_field(8, 0));
    header.replace(116, 8, octal_field(8, 0));
    header.replace(124, 12, octal_field(12, declared_size));
    header.replace(136, 12, octal_field(12, 0));
    header[156] = '0';
    set_bytes(&header, 257, 6, "ustar ");
    set_bytes(&header, 263, 2, " \0");
    write_checksum(&header);

    std::string archive;
    archive.append(header);
    archive.append(1024, '\0');
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
    TEST_ASSERT(archive.size() >= 512);

    const char* header = archive.data();
    std::size_t path_length = 0;
    while (path_length < 100 && header[path_length] != '\0') {
        ++path_length;
    }
    const std::string path(header, path_length);
    const char typeflag = header[156] == '\0' ? '0' : header[156];
    TEST_ASSERT(typeflag == '0');

    const std::uint64_t size = parse_octal_value(header + 124, 12);
    const std::size_t body_offset = 512;
    const std::size_t body_size = static_cast<std::size_t>(size);
    const std::size_t padded_size = ((body_size + 511) / 512) * 512;
    TEST_ASSERT(body_offset + padded_size <= archive.size());

    for (std::size_t offset = body_offset + padded_size; offset < archive.size(); offset += 512) {
        TEST_ASSERT(offset + 512 <= archive.size());
        TEST_ASSERT(block_is_zero(archive.data() + offset));
    }

    return std::make_pair(path, archive.substr(body_offset, body_size));
}

static std::vector<std::string> read_tar_paths(const std::string& archive) {
    std::vector<std::string> paths;
    std::size_t offset = 0;
    std::string long_name;

    while (offset + 512 <= archive.size() && !block_is_zero(archive.data() + offset)) {
        const char* header = archive.data() + offset;
        std::size_t path_length = 0;
        while (path_length < 100 && header[path_length] != '\0') {
            ++path_length;
        }
        std::string path(header, path_length);
        const char typeflag = header[156] == '\0' ? '0' : header[156];
        const std::uint64_t size = parse_octal_value(header + 124, 12);
        const std::size_t body_offset = offset + 512;
        const std::size_t padded_size = ((static_cast<std::size_t>(size) + 511) / 512) * 512;

        if (typeflag == 'L') {
            long_name = archive.substr(body_offset, static_cast<std::size_t>(size));
            while (!long_name.empty() && long_name[long_name.size() - 1] == '\0') {
                long_name.erase(long_name.size() - 1);
            }
            offset = body_offset + padded_size;
            continue;
        }

        if (!long_name.empty()) {
            path = long_name;
            long_name.clear();
        }
        paths.push_back(path);
        offset = body_offset + padded_size;
    }

    return paths;
}

static std::vector<std::pair<std::string, std::string> > read_tar_symlinks(const std::string& archive) {
    std::vector<std::pair<std::string, std::string> > symlinks;
    std::size_t offset = 0;
    std::string long_name;

    while (offset + 512 <= archive.size() && !block_is_zero(archive.data() + offset)) {
        const char* header = archive.data() + offset;
        std::size_t path_length = 0;
        while (path_length < 100 && header[path_length] != '\0') {
            ++path_length;
        }
        std::string path(header, path_length);
        const char typeflag = header[156] == '\0' ? '0' : header[156];
        const std::uint64_t size = parse_octal_value(header + 124, 12);
        const std::size_t body_offset = offset + 512;
        const std::size_t padded_size = ((static_cast<std::size_t>(size) + 511) / 512) * 512;

        if (typeflag == 'L') {
            long_name = archive.substr(body_offset, static_cast<std::size_t>(size));
            while (!long_name.empty() && long_name[long_name.size() - 1] == '\0') {
                long_name.erase(long_name.size() - 1);
            }
            offset = body_offset + padded_size;
            continue;
        }

        if (!long_name.empty()) {
            path = long_name;
            long_name.clear();
        }

        if (typeflag == '2') {
            std::size_t target_length = 0;
            while (target_length < 100 && header[157 + target_length] != '\0') {
                ++target_length;
            }
            symlinks.push_back(std::make_pair(path, std::string(header + 157, target_length)));
        }

        offset = body_offset + padded_size;
    }

    return symlinks;
}

#ifndef _WIN32
static std::uint64_t read_first_tar_mode(const std::string& archive) {
    TEST_ASSERT(archive.size() >= 512);
    return parse_octal_value(archive.data() + 100, 8);
}
#endif

static std::string replace_all(std::string value, const std::string& needle, const std::string& replacement) {
    std::string::size_type position = 0;
    while ((position = value.find(needle, position)) != std::string::npos) {
        value.replace(position, needle.size(), replacement);
        position += replacement.size();
    }
    return value;
}

static std::string apply_template(const std::string& raw, const fs::path& root) {
    return replace_all(raw, "{root}", root.string());
}

static bool case_applies_to_host(const Json& case_json) {
    if (!case_json.contains("platforms")) {
        return true;
    }
#ifdef _WIN32
    const std::string platform = "windows";
#else
    const std::string platform = "posix";
#endif
    const Json& platforms = case_json.at("platforms");
    for (Json::const_iterator it = platforms.begin(); it != platforms.end(); ++it) {
        if (it->get<std::string>() == platform) {
            return true;
        }
    }
    return false;
}

static void apply_setup(const fs::path& root, const Json& setup) {
    if (setup.is_null()) {
        return;
    }

    if (setup.contains("dirs")) {
        const Json& dirs = setup.at("dirs");
        for (Json::const_iterator it = dirs.begin(); it != dirs.end(); ++it) {
            fs::create_directories(apply_template(it->get<std::string>(), root));
        }
    }

    if (setup.contains("files")) {
        const Json& files = setup.at("files");
        for (Json::const_iterator it = files.begin(); it != files.end(); ++it) {
            const fs::path path = apply_template(it->at("path").get<std::string>(), root);
            fs::create_directories(path.parent_path());
            write_text(path, it->at("contents").get<std::string>());
        }
    }

#ifndef _WIN32
    if (setup.contains("symlinks")) {
        const Json& symlinks = setup.at("symlinks");
        for (Json::const_iterator it = symlinks.begin(); it != symlinks.end(); ++it) {
            const fs::path path = apply_template(it->at("path").get<std::string>(), root);
            fs::create_directories(path.parent_path());
            fs::create_symlink(apply_template(it->at("target").get<std::string>(), root), path);
        }
    }

    if (setup.contains("fifos")) {
        const Json& fifos = setup.at("fifos");
        for (Json::const_iterator it = fifos.begin(); it != fifos.end(); ++it) {
            const fs::path path = apply_template(it->get<std::string>(), root);
            fs::create_directories(path.parent_path());
            TEST_ASSERT(mkfifo(path.c_str(), 0600) == 0);
        }
    }
#endif
}

static TransferSourceType source_type_from_wire(const std::string& value) {
    TransferSourceType source_type = TransferSourceType::File;
    TEST_ASSERT(parse_transfer_source_type_wire_value(value, &source_type));
    return source_type;
}

static TransferOverwrite overwrite_from_wire(const std::string& value) {
    TransferOverwrite overwrite = TransferOverwrite::Fail;
    TEST_ASSERT(parse_transfer_overwrite_wire_value(value, &overwrite));
    return overwrite;
}

static TransferSymlinkMode symlink_mode_from_wire(const std::string& value) {
    TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve;
    TEST_ASSERT(parse_transfer_symlink_mode_wire_value(value, &symlink_mode));
    return symlink_mode;
}

static std::vector<std::string> sorted_strings(std::vector<std::string> values) {
    std::sort(values.begin(), values.end());
    return values;
}

static std::vector<std::string> warning_codes(const std::vector<TransferWarning>& warnings) {
    std::vector<std::string> codes;
    for (std::size_t i = 0; i < warnings.size(); ++i) {
        codes.push_back(warnings[i].code);
    }
    return sorted_strings(codes);
}

static std::vector<std::string> json_string_array(const Json& values) {
    std::vector<std::string> out;
    for (Json::const_iterator it = values.begin(); it != values.end(); ++it) {
        out.push_back(it->get<std::string>());
    }
    return out;
}

static void assert_string_vectors_equal(std::vector<std::string> actual, std::vector<std::string> expected) {
    actual = sorted_strings(actual);
    expected = sorted_strings(expected);
    TEST_ASSERT(actual.size() == expected.size());
    for (std::size_t i = 0; i < actual.size(); ++i) {
        TEST_ASSERT(actual[i] == expected[i]);
    }
}

static void assert_file_contents_match(const fs::path& root, const Json& files) {
    for (Json::const_iterator it = files.begin(); it != files.end(); ++it) {
        const fs::path path = apply_template(it->at("path").get<std::string>(), root);
        TEST_ASSERT(read_text(path) == it->at("contents").get<std::string>());
    }
}

static void assert_missing_paths_match(const fs::path& root, const Json& paths) {
    for (Json::const_iterator it = paths.begin(); it != paths.end(); ++it) {
        TEST_ASSERT(!fs::exists(apply_template(it->get<std::string>(), root)));
    }
}

#ifndef _WIN32
static void assert_symlink_targets_match(const fs::path& root, const Json& symlinks) {
    for (Json::const_iterator it = symlinks.begin(); it != symlinks.end(); ++it) {
        const fs::path path = apply_template(it->at("path").get<std::string>(), root);
        const fs::path expected = apply_template(it->at("target").get<std::string>(), root);
        TEST_ASSERT(fs::read_symlink(path) == expected);
    }
}
#else
static void assert_symlink_targets_match(const fs::path&, const Json& symlinks) {
    TEST_ASSERT(symlinks.empty());
}
#endif

static std::string build_archive_from_contract_entries(const Json& entries) {
    std::string archive;
    for (Json::const_iterator it = entries.begin(); it != entries.end(); ++it) {
        const std::string type = it->at("type").get<std::string>();
        if (type == "directory") {
            append_tar_directory(archive, it->at("path").get<std::string>());
            continue;
        }
        if (type == "file") {
            append_tar_file(archive, it->at("path").get<std::string>(), it->at("contents").get<std::string>());
            continue;
        }
        if (type == "symlink") {
            append_tar_symlink(
                &archive, it->at("path").get<std::string>(), it->at("target").get<std::string>());
            continue;
        }
        TEST_ASSERT(false);
    }
    finalize_tar(archive);
    return archive;
}

static fs::path roundtrip_destination_for_source_type(const fs::path& root, TransferSourceType source_type) {
    if (source_type == TransferSourceType::File) {
        return root / "roundtrip.txt";
    }
    return root / "roundtrip";
}

static void assert_transfer_type_wire_helpers() {
    TEST_ASSERT(std::string(transfer_source_type_wire_value(TransferSourceType::File)) == "file");
    TEST_ASSERT(std::string(transfer_source_type_wire_value(TransferSourceType::Directory)) == "directory");
    TEST_ASSERT(std::string(transfer_source_type_wire_value(TransferSourceType::Multiple)) == "multiple");
    TEST_ASSERT(std::string(transfer_symlink_mode_wire_value(TransferSymlinkMode::Preserve)) == "preserve");
    TEST_ASSERT(std::string(transfer_symlink_mode_wire_value(TransferSymlinkMode::Follow)) == "follow");
    TEST_ASSERT(std::string(transfer_symlink_mode_wire_value(TransferSymlinkMode::Skip)) == "skip");

    TransferSourceType parsed_source_type = TransferSourceType::File;
    TEST_ASSERT(parse_transfer_source_type_wire_value("directory", &parsed_source_type));
    TEST_ASSERT(parsed_source_type == TransferSourceType::Directory);
    TEST_ASSERT(!parse_transfer_source_type_wire_value("folder", &parsed_source_type));

    TransferSymlinkMode parsed_symlink_mode = TransferSymlinkMode::Preserve;
    TEST_ASSERT(parse_transfer_symlink_mode_wire_value("skip", &parsed_symlink_mode));
    TEST_ASSERT(parsed_symlink_mode == TransferSymlinkMode::Skip);
    TEST_ASSERT(!parse_transfer_symlink_mode_wire_value("copy", &parsed_symlink_mode));

    TEST_ASSERT(std::string(transfer_overwrite_wire_value(TransferOverwrite::Replace)) == "replace");

    TransferOverwrite parsed_overwrite = TransferOverwrite::Fail;
    TEST_ASSERT(parse_transfer_overwrite_wire_value("merge", &parsed_overwrite));
    TEST_ASSERT(parsed_overwrite == TransferOverwrite::Merge);
    TEST_ASSERT(!parse_transfer_overwrite_wire_value("copy", &parsed_overwrite));
}

static void assert_file_transfer() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-file";
    fs::remove_all(root);
    fs::create_directories(root);

    write_text(root / "source.txt", "hello transfer");
    const ExportedPayload exported = export_path((root / "source.txt").string());
    TEST_ASSERT(exported.source_type == TransferSourceType::File);
    const std::pair<std::string, std::string> file_entry = read_single_file_tar(exported.bytes);
    TEST_ASSERT(file_entry.first == SINGLE_FILE_ENTRY);
    TEST_ASSERT(file_entry.second == "hello transfer");

    const ImportSummary imported =
        import_path(exported.bytes, exported.source_type, (root / "copied.txt").string(), TransferOverwrite::Replace, true);
    TEST_ASSERT(imported.files_copied == 1);
    TEST_ASSERT(imported.directories_copied == 0);
    TEST_ASSERT(read_text(root / "copied.txt") == "hello transfer");
}

static void assert_file_transfer_blocks_unexpected_entry_path() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-file-entry-path";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)import_path(tar_with_single_file("payload.txt", "bad"),
                          TransferSourceType::File,
                          (root / "copied.txt").string(),
                          TransferOverwrite::Replace,
                          true);
    } catch (...) {
        rejected = true;
    }

    TEST_ASSERT(rejected);
}

static void assert_file_transfer_blocks_raw_bytes() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-file-raw";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)import_path("raw-bytes", TransferSourceType::File, (root / "copied.txt").string(), TransferOverwrite::Replace, true);
    } catch (...) {
        rejected = true;
    }

    TEST_ASSERT(rejected);
}

static void assert_transfer_rejects_entry_size_over_limit() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-size-limit";
    fs::remove_all(root);
    fs::create_directories(root);

    TransferLimitConfig limits;
    limits.max_archive_bytes = 4096ULL;
    limits.max_entry_bytes = 8ULL;

    bool rejected = false;
    try {
        (void)import_path(tar_with_single_file(SINGLE_FILE_ENTRY, "0123456789"),
                          TransferSourceType::File,
                          (root / "dest.txt").string(),
                          TransferOverwrite::Replace,
                          true,
                          TransferSymlinkMode::Preserve,
                          limits);
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("transfer entry limit") != std::string::npos;
    }
    TEST_ASSERT(rejected);
    TEST_ASSERT(!fs::exists(root / "dest.txt"));
}

static void assert_transfer_rejects_unrepresentable_tar_size() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-huge-size";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)import_path(tar_with_declared_file_size(SINGLE_FILE_ENTRY, 077777777777ULL),
                          TransferSourceType::File,
                          (root / "dest.txt").string(),
                          TransferOverwrite::Replace,
                          true);
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("too large") != std::string::npos ||
                   failure.message.find("limit") != std::string::npos;
    }
    TEST_ASSERT(rejected);
    TEST_ASSERT(!fs::exists(root / "dest.txt"));
}

static void assert_transfer_rejects_summary_size_over_limit() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-summary-limit";
    fs::remove_all(root);

    std::string archive;
    append_tar_directory(archive, ".");
    append_tar_entry(&archive, TRANSFER_SUMMARY_ENTRY, '0', "{\"warnings\":[]}");
    finalize_tar(archive);

    TransferLimitConfig limits;
    limits.max_archive_bytes = 4096ULL;
    limits.max_entry_bytes = 8ULL;

    bool rejected = false;
    try {
        (void)import_path(archive,
                          TransferSourceType::Directory,
                          (root / "dest").string(),
                          TransferOverwrite::Replace,
                          true,
                          TransferSymlinkMode::Preserve,
                          limits);
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("transfer entry limit") != std::string::npos;
    }
    TEST_ASSERT(rejected);
    TEST_ASSERT(!fs::exists(root / "dest" / TRANSFER_SUMMARY_ENTRY));
}

static void assert_directory_round_trip() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-dir";
    fs::remove_all(root);
    fs::create_directories(root / "source" / "nested" / "empty");
    write_text(root / "source" / "nested" / "hello.txt", "hello directory");
    write_text(root / "source" / "top.txt", "top level");

    const ExportedPayload exported = export_path((root / "source").string());
    TEST_ASSERT(exported.source_type == TransferSourceType::Directory);

    const ImportSummary imported =
        import_path(exported.bytes, exported.source_type, (root / "dest").string(), TransferOverwrite::Replace, true);

    TEST_ASSERT(imported.source_type == TransferSourceType::Directory);
    TEST_ASSERT(imported.files_copied == 2);
    TEST_ASSERT(imported.directories_copied >= 3);
    TEST_ASSERT(read_text(root / "dest" / "nested" / "hello.txt") == "hello directory");
    TEST_ASSERT(read_text(root / "dest" / "top.txt") == "top level");
    TEST_ASSERT(fs::is_directory(root / "dest" / "nested" / "empty"));
}

static void assert_directory_replace_behavior() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-replace";
    fs::remove_all(root);
    fs::create_directories(root / "source");
    fs::create_directories(root / "dest" / "stale");
    write_text(root / "source" / "fresh.txt", "fresh");
    write_text(root / "dest" / "stale" / "old.txt", "old");

    const ExportedPayload exported = export_path((root / "source").string());
    const ImportSummary imported =
        import_path(exported.bytes, exported.source_type, (root / "dest").string(), TransferOverwrite::Replace, true);

    TEST_ASSERT(imported.replaced);
    TEST_ASSERT(!fs::exists(root / "dest" / "stale" / "old.txt"));
    TEST_ASSERT(read_text(root / "dest" / "fresh.txt") == "fresh");
}

static void assert_path_info_reports_existing_directory() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-path-info";
    fs::remove_all(root);
    fs::create_directories(root / "dest");

    const PathInfo existing = path_info((root / "dest").string());
    TEST_ASSERT(existing.exists);
    TEST_ASSERT(existing.is_directory);

    const PathInfo missing = path_info((root / "missing").string());
    TEST_ASSERT(!missing.exists);
    TEST_ASSERT(!missing.is_directory);
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
    const ImportSummary imported =
        import_path(exported.bytes, exported.source_type, (root / "dest").string(), TransferOverwrite::Merge, true);

    TEST_ASSERT(!imported.replaced);
    TEST_ASSERT(read_text(root / "dest" / "nested" / "fresh.txt") == "fresh");
    TEST_ASSERT(read_text(root / "dest" / "stale.txt") == "stale");
    TEST_ASSERT(read_text(root / "dest" / "nested" / "old.txt") == "old");
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
    const ImportSummary imported =
        import_path(exported.bytes, exported.source_type, (root / "dest").string(), TransferOverwrite::Replace, true);

    TEST_ASSERT(imported.source_type == TransferSourceType::Directory);
    TEST_ASSERT(read_text(root / "dest" / long_name / "nested" / "payload.txt") == "long path");
}

static void assert_directory_export_excludes_matching_entries() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-exclude-dir";
    fs::remove_all(root);
    fs::create_directories(root / "source" / ".git");
    fs::create_directories(root / "source" / "logs");
    fs::create_directories(root / "source" / "src");
    write_text(root / "source" / "keep.txt", "keep");
    write_text(root / "source" / "top.log", "drop");
    write_text(root / "source" / ".git" / "config", "secret");
    write_text(root / "source" / "logs" / "readme.txt", "keep");
    write_text(root / "source" / "logs" / "app.log", "drop");
    write_text(root / "source" / "src" / "a.cpp", "drop");
    write_text(root / "source" / "src" / "z.cpp", "keep");

    std::vector<std::string> exclude;
    exclude.push_back("**/*.log");
    exclude.push_back(".git/**");
    exclude.push_back("src/[ab].cpp");
    const ExportedPayload exported = export_path((root / "source").string(), TransferSymlinkMode::Preserve, exclude);
    const std::vector<std::string> archive_paths = read_tar_paths(exported.bytes);

    TEST_ASSERT(std::find(archive_paths.begin(), archive_paths.end(), ".") != archive_paths.end());
    TEST_ASSERT(std::find(archive_paths.begin(), archive_paths.end(), "keep.txt") != archive_paths.end());
    TEST_ASSERT(std::find(archive_paths.begin(), archive_paths.end(), "logs/readme.txt") != archive_paths.end());
    TEST_ASSERT(std::find(archive_paths.begin(), archive_paths.end(), "src/z.cpp") != archive_paths.end());
    TEST_ASSERT(std::find(archive_paths.begin(), archive_paths.end(), "top.log") == archive_paths.end());
    TEST_ASSERT(std::find(archive_paths.begin(), archive_paths.end(), ".git") == archive_paths.end());
    TEST_ASSERT(std::find(archive_paths.begin(), archive_paths.end(), ".git/config") == archive_paths.end());
    TEST_ASSERT(std::find(archive_paths.begin(), archive_paths.end(), "logs/app.log") == archive_paths.end());
    TEST_ASSERT(std::find(archive_paths.begin(), archive_paths.end(), "src/a.cpp") == archive_paths.end());

    const ImportSummary imported =
        import_path(exported.bytes, exported.source_type, (root / "dest").string(), TransferOverwrite::Replace, true);
    TEST_ASSERT(imported.files_copied == 3);
    TEST_ASSERT(imported.warnings.empty());
    TEST_ASSERT(read_text(root / "dest" / "keep.txt") == "keep");
    TEST_ASSERT(read_text(root / "dest" / "logs" / "readme.txt") == "keep");
    TEST_ASSERT(read_text(root / "dest" / "src" / "z.cpp") == "keep");
    TEST_ASSERT(!fs::exists(root / "dest" / "top.log"));
    TEST_ASSERT(!fs::exists(root / "dest" / ".git"));
    TEST_ASSERT(!fs::exists(root / "dest" / "logs" / "app.log"));
    TEST_ASSERT(!fs::exists(root / "dest" / "src" / "a.cpp"));
}

static void assert_single_file_export_ignores_exclude_patterns() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-exclude-file";
    fs::remove_all(root);
    fs::create_directories(root);
    write_text(root / "hello.txt", "hello");

    std::vector<std::string> exclude;
    exclude.push_back("**/*.txt");
    const ExportedPayload exported = export_path((root / "hello.txt").string(), TransferSymlinkMode::Preserve, exclude);

    TEST_ASSERT(exported.source_type == TransferSourceType::File);
    const std::pair<std::string, std::string> file_entry = read_single_file_tar(exported.bytes);
    TEST_ASSERT(file_entry.first == SINGLE_FILE_ENTRY);
    TEST_ASSERT(file_entry.second == "hello");
}

#ifndef _WIN32
static void assert_symlink_sources_are_preserved_by_default() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-preserve";
    fs::remove_all(root);
    fs::create_directories(root / "source");
    write_text(root / "source" / "regular.txt", "regular");
    fs::create_symlink("regular.txt", root / "source" / "link.txt");

    const ExportedPayload exported = export_path((root / "source").string());
    TEST_ASSERT(exported.source_type == TransferSourceType::Directory);
    TEST_ASSERT(exported.bytes.find("link.txt") != std::string::npos);
}

static void assert_top_level_file_symlink_can_be_followed() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-follow-file";
    fs::remove_all(root);
    fs::create_directories(root);
    write_text(root / "target.txt", "target");
    fs::create_symlink(root / "target.txt", root / "link.txt");

    const ExportedPayload exported = export_path((root / "link.txt").string(), TransferSymlinkMode::Follow);
    TEST_ASSERT(exported.source_type == TransferSourceType::File);

    const ImportSummary imported =
        import_path(exported.bytes, exported.source_type, (root / "dest.txt").string(), TransferOverwrite::Replace, true);
    TEST_ASSERT(imported.files_copied == 1);
    TEST_ASSERT(read_text(root / "dest.txt") == "target");
}

static void assert_top_level_symlink_is_preserved_without_following_target() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-preserve-root";
    fs::remove_all(root);
    fs::create_directories(root);
    fs::create_symlink("missing-target.txt", root / "broken-link.txt");

    const ExportedPayload exported = export_path((root / "broken-link.txt").string(), TransferSymlinkMode::Preserve);
    TEST_ASSERT(exported.source_type == TransferSourceType::File);

    const ImportSummary imported =
        import_path(exported.bytes, exported.source_type, (root / "restored-link.txt").string(), TransferOverwrite::Replace, true);
    TEST_ASSERT(imported.files_copied == 1);
    TEST_ASSERT(fs::read_symlink(root / "restored-link.txt") == fs::path("missing-target.txt"));
}

static void assert_executable_bits_round_trip() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-executable";
    fs::remove_all(root);
    fs::create_directories(root);
    const fs::path source = root / "tool.sh";
    write_text(source, "#!/bin/sh\necho hi\n");
    fs::permissions(
        source, fs::perms::owner_exec | fs::perms::group_exec | fs::perms::others_exec, fs::perm_options::add);

    const ExportedPayload exported = export_path(source.string());
    TEST_ASSERT((read_first_tar_mode(exported.bytes) & 0111) == 0111);

    const fs::path imported_path = root / "imported.sh";
    const ImportSummary imported =
        import_path(exported.bytes, exported.source_type, imported_path.string(), TransferOverwrite::Replace, true);
    TEST_ASSERT(imported.files_copied == 1);
    TEST_ASSERT((static_cast<unsigned>(fs::status(imported_path).permissions()) &
            static_cast<unsigned>(fs::perms::owner_exec | fs::perms::group_exec | fs::perms::others_exec)) != 0U);

    std::string archive;
    append_tar_directory(archive, ".");
    append_tar_file_with_mode(archive, "bin/tool.sh", "#!/bin/sh\necho hi\n", 0755);
    finalize_tar(archive);
    const fs::path directory_dest = root / "directory-dest";
    const ImportSummary directory_imported =
        import_path(archive, TransferSourceType::Directory, directory_dest.string(), TransferOverwrite::Replace, true);
    TEST_ASSERT(directory_imported.files_copied == 1);
    TEST_ASSERT((static_cast<unsigned>(fs::status(directory_dest / "bin" / "tool.sh").permissions()) &
            static_cast<unsigned>(fs::perms::owner_exec | fs::perms::group_exec | fs::perms::others_exec)) != 0U);
}

static void assert_transfer_skips_special_files_with_warning() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-special-skip";
    fs::remove_all(root);
    fs::create_directories(root / "source");
    write_text(root / "source" / "regular.txt", "regular");
    const fs::path fifo = root / "source" / "events.fifo";
    TEST_ASSERT(mkfifo(fifo.c_str(), 0600) == 0);

    const ExportedPayload exported = export_path((root / "source").string());
    TEST_ASSERT(exported.source_type == TransferSourceType::Directory);
    TEST_ASSERT(exported.bytes.find("regular.txt") != std::string::npos);
    TEST_ASSERT(exported.bytes.find(TRANSFER_SUMMARY_ENTRY) != std::string::npos);

    const ImportSummary imported =
        import_path(exported.bytes, TransferSourceType::Directory, (root / "dest").string(), TransferOverwrite::Replace, true);
    TEST_ASSERT(imported.warnings.size() == 1);
    TEST_ASSERT(imported.warnings[0].code == "transfer_skipped_unsupported_entry");
    TEST_ASSERT(read_text(root / "dest" / "regular.txt") == "regular");
    TEST_ASSERT(!fs::exists(root / "dest" / "events.fifo"));
    TEST_ASSERT(!fs::exists(root / "dest" / TRANSFER_SUMMARY_ENTRY));
}

static void assert_top_level_special_files_are_unsupported() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-top-special";
    fs::remove_all(root);
    fs::create_directories(root);
    const fs::path socket_path = root / "events.sock";

    const int socket_fd = socket(AF_UNIX, SOCK_STREAM, 0);
    TEST_ASSERT(socket_fd >= 0);
    sockaddr_un address;
    std::memset(&address, 0, sizeof(address));
    address.sun_family = AF_UNIX;
    const std::string socket_path_text = socket_path.string();
    TEST_ASSERT(socket_path_text.size() < sizeof(address.sun_path));
    std::strncpy(address.sun_path, socket_path_text.c_str(), sizeof(address.sun_path) - 1);
    TEST_ASSERT(bind(socket_fd, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0);

    bool rejected = false;
    try {
        (void)export_path(socket_path_text);
    } catch (const std::exception& ex) {
        rejected = std::string(ex.what()).find("regular file or directory") != std::string::npos;
    }
    close(socket_fd);
    TEST_ASSERT(rejected);
}

static void assert_symlink_import_preserves_links() {
    std::string archive;
    append_tar_file(archive, "alpha.txt", "alpha");
    append_tar_symlink(&archive, "alpha-link", "alpha.txt");
    finalize_tar(archive);

    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-import";
    fs::remove_all(root);

    const ImportSummary imported =
        import_path(archive, TransferSourceType::Directory, (root / "dest").string(), TransferOverwrite::Replace, true);

    TEST_ASSERT(imported.files_copied == 2);
    TEST_ASSERT(read_text(root / "dest" / "alpha.txt") == "alpha");
    TEST_ASSERT(fs::read_symlink(root / "dest" / "alpha-link") == fs::path("alpha.txt"));
}

static void assert_symlink_import_skip_reports_warning() {
    std::string archive;
    append_tar_file(archive, "alpha.txt", "alpha");
    append_tar_symlink(&archive, "alpha-link", "alpha.txt");
    finalize_tar(archive);

    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-import-skip";
    fs::remove_all(root);

    const ImportSummary imported = import_path(
        archive, TransferSourceType::Directory, (root / "dest").string(), TransferOverwrite::Replace, true, TransferSymlinkMode::Skip);

    TEST_ASSERT(imported.files_copied == 1);
    TEST_ASSERT(imported.warnings.size() == 1);
    TEST_ASSERT(imported.warnings[0].code == "transfer_skipped_symlink");
    TEST_ASSERT(read_text(root / "dest" / "alpha.txt") == "alpha");
    TEST_ASSERT(!fs::exists(root / "dest" / "alpha-link"));

    std::string file_archive;
    append_tar_symlink(&file_archive, SINGLE_FILE_ENTRY, "missing-target.txt");
    finalize_tar(file_archive);
    const ImportSummary file_imported = import_path(file_archive,
                                                    TransferSourceType::File,
                                                    (root / "skipped-file-link").string(),
                                                    TransferOverwrite::Replace,
                                                    true,
                                                    TransferSymlinkMode::Skip);
    TEST_ASSERT(file_imported.files_copied == 0);
    TEST_ASSERT(file_imported.warnings.size() == 1);
    TEST_ASSERT(file_imported.warnings[0].code == "transfer_skipped_symlink");
    TEST_ASSERT(!fs::exists(root / "skipped-file-link"));
}

static void assert_symlink_import_rejects_absolute_target() {
    std::string archive;
    append_tar_symlink(&archive, "bad-link", "/etc/passwd");
    finalize_tar(archive);

    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-absolute";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)import_path(archive, TransferSourceType::Directory, (root / "dest").string(), TransferOverwrite::Replace, true);
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("symlink target") != std::string::npos;
    }
    TEST_ASSERT(rejected);
    TEST_ASSERT(!fs::exists(root / "dest" / "bad-link"));
}

static void assert_symlink_import_rejects_parent_target() {
    std::string archive;
    append_tar_symlink(&archive, "bad-link", "../escape.txt");
    finalize_tar(archive);

    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-parent";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)import_path(archive, TransferSourceType::Directory, (root / "dest").string(), TransferOverwrite::Replace, true);
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("symlink target") != std::string::npos;
    }
    TEST_ASSERT(rejected);
    TEST_ASSERT(!fs::exists(root / "dest" / "bad-link"));
}
#endif

#ifdef _WIN32
static void assert_windows_symlink_import_modes_skip_with_warning() {
    const TransferSymlinkMode modes[] = {
        TransferSymlinkMode::Preserve,
        TransferSymlinkMode::Follow,
        TransferSymlinkMode::Skip,
    };
    for (std::size_t i = 0; i < sizeof(modes) / sizeof(modes[0]); ++i) {
        std::string archive;
        append_tar_file(archive, "alpha.txt", "alpha");
        append_tar_symlink(&archive, "alpha-link", "alpha.txt");
        finalize_tar(archive);

        const fs::path root = fs::temp_directory_path() / ("remote-exec-cpp-transfer-win-symlink-import-" +
                                                           std::string(transfer_symlink_mode_wire_value(modes[i])));
        fs::remove_all(root);

        const ImportSummary imported =
            import_path(archive, TransferSourceType::Directory, (root / "dest").string(), TransferOverwrite::Replace, true, modes[i]);

        TEST_ASSERT(imported.files_copied == 1);
        TEST_ASSERT(imported.warnings.size() == 1);
        TEST_ASSERT(imported.warnings[0].code == "transfer_skipped_symlink");
        TEST_ASSERT(read_text(root / "dest" / "alpha.txt") == "alpha");
        TEST_ASSERT(!fs::exists(root / "dest" / "alpha-link"));

        std::string file_archive;
        append_tar_symlink(&file_archive, SINGLE_FILE_ENTRY, "missing-target.txt");
        finalize_tar(file_archive);

        const ImportSummary file_imported = import_path(
            file_archive, TransferSourceType::File, (root / "skipped-file-link").string(), TransferOverwrite::Replace, true, modes[i]);

        TEST_ASSERT(file_imported.files_copied == 0);
        TEST_ASSERT(file_imported.warnings.size() == 1);
        TEST_ASSERT(file_imported.warnings[0].code == "transfer_skipped_symlink");
        TEST_ASSERT(!fs::exists(root / "skipped-file-link"));
    }
}
#endif

static bool directory_import_rejects_path(const std::string& path) {
    const std::string archive = tar_with_single_file(path, "bad");
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-traversal";
    fs::remove_all(root);

    bool rejected = false;
    try {
        (void)import_path(archive, TransferSourceType::Directory, (root / "dest").string(), TransferOverwrite::Replace, true);
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("archive path") != std::string::npos ||
                   failure.message.find("escapes destination") != std::string::npos;
    }

    TEST_ASSERT(!fs::exists(root / "escape.txt"));
    TEST_ASSERT(!fs::exists(root / "dest" / "escape.txt"));
    return rejected;
}

static void assert_directory_traversal_is_rejected() {
    TEST_ASSERT(directory_import_rejects_path("../escape.txt"));
    TEST_ASSERT(directory_import_rejects_path("foo/../../../etc/shadow"));
    TEST_ASSERT(directory_import_rejects_path("safe/../escape.txt"));
    TEST_ASSERT(directory_import_rejects_path("safe/./escape.txt"));
    TEST_ASSERT(directory_import_rejects_path("safe//escape.txt"));
    TEST_ASSERT(directory_import_rejects_path("safe\\..\\escape.txt"));
    TEST_ASSERT(directory_import_rejects_path("safe\\.\\escape.txt"));

    std::string long_name_archive;
    append_gnu_long_name(&long_name_archive, "safe/../../escape.txt");
    append_tar_entry(&long_name_archive, "ignored", '0', "bad");
    finalize_tar(long_name_archive);

    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-long-name-traversal";
    fs::remove_all(root);
    bool rejected = false;
    try {
        (void)import_path(long_name_archive, TransferSourceType::Directory, (root / "dest").string(), TransferOverwrite::Replace, true);
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("archive path") != std::string::npos ||
                   failure.message.find("escapes destination") != std::string::npos;
    }

    TEST_ASSERT(rejected);
    TEST_ASSERT(!fs::exists(root / "escape.txt"));
    TEST_ASSERT(!fs::exists(root / "dest" / "escape.txt"));
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
        import_path(archive, TransferSourceType::Multiple, (root / "dest").string(), TransferOverwrite::Replace, true);

    TEST_ASSERT(imported.source_type == TransferSourceType::Multiple);
    TEST_ASSERT(imported.files_copied == 2);
    TEST_ASSERT(imported.directories_copied >= 2);
    TEST_ASSERT(read_text(root / "dest" / "alpha.txt") == "alpha");
    TEST_ASSERT(read_text(root / "dest" / "nested" / "beta.txt") == "beta");
}

static void assert_shared_transfer_contract_cases() {
    const Json& contract = test_contract::transfer_semantics_contract();

    const Json& import_cases = contract.at("import_cases");
    for (Json::const_iterator it = import_cases.begin(); it != import_cases.end(); ++it) {
        if (!case_applies_to_host(*it)) {
            continue;
        }

        const fs::path root = fs::temp_directory_path() /
                              ("remote-exec-cpp-transfer-contract-import-" + it->at("name").get<std::string>());
        fs::remove_all(root);
        fs::create_directories(root);
        apply_setup(root, it->contains("setup") ? it->at("setup") : Json());

        const std::string archive = build_archive_from_contract_entries(it->at("archive_entries"));
        const fs::path destination = apply_template(it->at("destination_path").get<std::string>(), root);
        const Json& expected = it->at("expected");

        if (expected.contains("error_message_fragment")) {
            bool rejected = false;
            try {
                (void)import_path(archive,
                                  source_type_from_wire(it->at("source_type").get<std::string>()),
                                  destination.string(),
                                  overwrite_from_wire(it->at("overwrite").get<std::string>()),
                                  it->at("create_parent").get<bool>(),
                                  symlink_mode_from_wire(it->at("symlink_mode").get<std::string>()));
            } catch (const TransferFailure& failure) {
                rejected = failure.message.find(expected.at("error_message_fragment").get<std::string>()) !=
                           std::string::npos;
            }
            TEST_ASSERT(rejected);
            continue;
        }

        const ImportSummary imported = import_path(archive,
                                                   source_type_from_wire(it->at("source_type").get<std::string>()),
                                                   destination.string(),
                                                   overwrite_from_wire(it->at("overwrite").get<std::string>()),
                                                   it->at("create_parent").get<bool>(),
                                                   symlink_mode_from_wire(it->at("symlink_mode").get<std::string>()));

        if (expected.contains("replaced")) {
            TEST_ASSERT(imported.replaced == expected.at("replaced").get<bool>());
        }
        if (expected.contains("files_copied")) {
            TEST_ASSERT(imported.files_copied == expected.at("files_copied").get<std::uint64_t>());
        }
        if (expected.contains("directories_copied_at_least")) {
            TEST_ASSERT(imported.directories_copied >= expected.at("directories_copied_at_least").get<std::uint64_t>());
        }
        assert_string_vectors_equal(warning_codes(imported.warnings), json_string_array(expected.at("warning_codes")));
        if (expected.contains("file_contents")) {
            assert_file_contents_match(root, expected.at("file_contents"));
        }
        if (expected.contains("missing_paths")) {
            assert_missing_paths_match(root, expected.at("missing_paths"));
        }
        if (expected.contains("symlink_targets")) {
            assert_symlink_targets_match(root, expected.at("symlink_targets"));
        }
    }

    const Json& export_cases = contract.at("export_cases");
    for (Json::const_iterator it = export_cases.begin(); it != export_cases.end(); ++it) {
        if (!case_applies_to_host(*it)) {
            continue;
        }

        const fs::path root = fs::temp_directory_path() /
                              ("remote-exec-cpp-transfer-contract-export-" + it->at("name").get<std::string>());
        fs::remove_all(root);
        fs::create_directories(root);
        apply_setup(root, it->contains("setup") ? it->at("setup") : Json());

        const ExportedPayload exported =
            export_path(apply_template(it->at("path").get<std::string>(), root),
                        symlink_mode_from_wire(it->at("symlink_mode").get<std::string>()));
        const Json& expected = it->at("expected");

        TEST_ASSERT(transfer_source_type_wire_value(exported.source_type) == expected.at("source_type").get<std::string>());
        const std::vector<std::string> archive_paths = read_tar_paths(exported.bytes);
        assert_string_vectors_equal(archive_paths, json_string_array(expected.at("archive_paths")));

        if (expected.contains("missing_archive_paths")) {
            const Json& missing_archive_paths = expected.at("missing_archive_paths");
            for (Json::const_iterator missing = missing_archive_paths.begin(); missing != missing_archive_paths.end();
                 ++missing) {
                TEST_ASSERT(std::find(archive_paths.begin(), archive_paths.end(), missing->get<std::string>()) ==
                            archive_paths.end());
            }
        }

        std::vector<std::string> actual_archive_symlinks;
        const std::vector<std::pair<std::string, std::string> > symlinks = read_tar_symlinks(exported.bytes);
        for (std::size_t i = 0; i < symlinks.size(); ++i) {
            actual_archive_symlinks.push_back(symlinks[i].first + "\n" + symlinks[i].second);
        }

        std::vector<std::string> expected_archive_symlinks;
        if (expected.contains("archive_symlinks")) {
            const Json& archive_symlinks = expected.at("archive_symlinks");
            for (Json::const_iterator link = archive_symlinks.begin(); link != archive_symlinks.end(); ++link) {
                expected_archive_symlinks.push_back(
                    link->at("path").get<std::string>() + "\n" + link->at("target").get<std::string>());
            }
        }
        assert_string_vectors_equal(actual_archive_symlinks, expected_archive_symlinks);

        const ImportSummary roundtrip = import_path(exported.bytes,
                                                    exported.source_type,
                                                    roundtrip_destination_for_source_type(root, exported.source_type)
                                                        .string(),
                                                    TransferOverwrite::Replace,
                                                    true,
                                                    symlink_mode_from_wire(it->at("symlink_mode").get<std::string>()));

        assert_string_vectors_equal(
            warning_codes(roundtrip.warnings), json_string_array(expected.at("roundtrip_warning_codes")));
        if (expected.contains("roundtrip_file_contents")) {
            assert_file_contents_match(root, expected.at("roundtrip_file_contents"));
        }
        if (expected.contains("roundtrip_missing_paths")) {
            assert_missing_paths_match(root, expected.at("roundtrip_missing_paths"));
        }
        if (expected.contains("roundtrip_symlink_targets")) {
            assert_symlink_targets_match(root, expected.at("roundtrip_symlink_targets"));
        }
    }
}

int main() {
    TEST_ASSERT(std::string(transfer_error_code_name(TransferRpcCode::SourceMissing)) == "transfer_source_missing");
    TEST_ASSERT(std::string(transfer_error_code_name(TransferRpcCode::CompressionUnsupported)) ==
           "transfer_compression_unsupported");
    TEST_ASSERT(std::string(transfer_error_code_name(TransferRpcCode::Internal)) == "internal_error");
    TEST_ASSERT(transfer_error_status(TransferRpcCode::Internal) == 500);
    TEST_ASSERT(transfer_error_status(TransferRpcCode::SourceMissing) == 400);
    TEST_ASSERT(std::string(image_error_code_name(ImageRpcCode::Internal)) == "internal_error");
    TEST_ASSERT(image_error_status(ImageRpcCode::Internal) == 500);
    TEST_ASSERT(image_error_status(ImageRpcCode::DecodeFailed) == 400);

    assert_transfer_type_wire_helpers();
    assert_file_transfer();
    assert_file_transfer_blocks_unexpected_entry_path();
    assert_file_transfer_blocks_raw_bytes();
    assert_transfer_rejects_entry_size_over_limit();
    assert_transfer_rejects_unrepresentable_tar_size();
    assert_transfer_rejects_summary_size_over_limit();
    assert_directory_round_trip();
    assert_directory_replace_behavior();
    assert_path_info_reports_existing_directory();
    assert_directory_merge_behavior();
    assert_directory_long_path_round_trip();
    assert_directory_export_excludes_matching_entries();
    assert_single_file_export_ignores_exclude_patterns();
#ifndef _WIN32
    assert_symlink_sources_are_preserved_by_default();
    assert_top_level_file_symlink_can_be_followed();
    assert_top_level_symlink_is_preserved_without_following_target();
    assert_executable_bits_round_trip();
    assert_transfer_skips_special_files_with_warning();
    assert_top_level_special_files_are_unsupported();
    assert_symlink_import_preserves_links();
    assert_symlink_import_skip_reports_warning();
    assert_symlink_import_rejects_absolute_target();
    assert_symlink_import_rejects_parent_target();
#endif
#ifdef _WIN32
    assert_windows_symlink_import_modes_skip_with_warning();
#endif
    assert_directory_traversal_is_rejected();
    assert_multiple_sources_import();
    assert_shared_transfer_contract_cases();
    return 0;
}
