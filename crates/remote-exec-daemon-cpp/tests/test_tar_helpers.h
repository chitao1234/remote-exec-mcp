#pragma once

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <string>

#include "codec/base64_codec.h"
#include "http/http_helpers.h"
#include "rpc/server_contract.h"
#include "test_assert.h"
#include "test_filesystem.h"

namespace test_tar {

inline std::string octal_field(std::size_t width, std::uint64_t value) {
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
    const std::size_t used = std::min(width - 1, digits.size());
    field.replace(width - 1 - used, used, digits.substr(digits.size() - used));
    field[width - 1] = ' ';
    return field;
}

inline void set_bytes(
    std::string* header,
    std::size_t offset,
    std::size_t width,
    const std::string& value
) {
    header->replace(offset, std::min(width, value.size()), value.substr(0, width));
}

inline void write_checksum(std::string* header) {
    std::fill(header->begin() + 148, header->begin() + 156, ' ');
    unsigned int checksum = 0;
    for (std::size_t i = 0; i < header->size(); ++i) {
        checksum += static_cast<unsigned char>((*header)[i]);
    }
    header->replace(148, 8, octal_field(8, checksum));
}

inline std::uint64_t parse_octal_value(const char* data, std::size_t size) {
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

inline void append_padded_bytes(std::string* archive, const std::string& bytes) {
    archive->append(bytes);
    const std::size_t remainder = bytes.size() % 512;
    if (remainder != 0) {
        archive->append(512 - remainder, '\0');
    }
}

inline void append_tar_entry(
    std::string* archive,
    const std::string& path,
    char typeflag,
    const std::string& body,
    std::uint64_t mode = 0644
) {
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

inline void append_gnu_long_name(std::string* archive, const std::string& path) {
    append_tar_entry(archive, "././@LongLink", 'L', path + '\0');
}

inline void append_tar_file(
    std::string& archive,
    const std::string& path,
    const std::string& body
) {
    if (path.size() >= 100) {
        append_gnu_long_name(&archive, path);
    }
    append_tar_entry(&archive, path, '0', body);
}

#ifndef _WIN32
inline void append_tar_file_with_mode(
    std::string& archive,
    const std::string& path,
    const std::string& body,
    std::uint64_t mode
) {
    if (path.size() >= 100) {
        append_gnu_long_name(&archive, path);
    }
    append_tar_entry(&archive, path, '0', body, mode);
}
#endif

inline void append_tar_directory(std::string& archive, const std::string& path) {
    if (path.size() >= 100) {
        append_gnu_long_name(&archive, path);
    }
    append_tar_entry(&archive, path, '5', "");
}

inline void append_tar_symlink(
    std::string& archive,
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

    archive.append(header);
}

inline void finalize_tar(std::string& archive) {
    archive.append(1024, '\0');
}

inline void append_u64_be(std::string* output, std::uint64_t value) {
    for (std::size_t i = 0; i < 8U; ++i) {
        output->push_back(static_cast<char>((value >> ((7U - i) * 8U)) & 0xffU));
    }
}

inline std::uint64_t read_u64_be(const std::string& value, std::size_t offset) {
    std::uint64_t output = 0U;
    for (std::size_t i = 0; i < 8U; ++i) {
        output = (output << 8U) | static_cast<unsigned char>(value[offset + i]);
    }
    return output;
}

inline std::string transfer_frame(unsigned char frame_type, const std::string& payload) {
    std::string output;
    output.push_back(static_cast<char>(frame_type));
    output.append(3, '\0');
    append_u64_be(&output, static_cast<std::uint64_t>(payload.size()));
    output.append(payload);
    return output;
}

inline std::string framed_transfer_body(const std::string& archive) {
    std::string output(
        server_contract::TRANSFER_STREAM_PREFACE,
        server_contract::TRANSFER_STREAM_PREFACE_LEN
    );
    output += transfer_frame(0x01, archive);
    output += transfer_frame(0x02, Json{{"archive_bytes", archive.size()}}.dump());
    return output;
}

inline std::string decode_framed_transfer_archive(const std::string& body) {
    TEST_ASSERT(
        body.compare(
            0,
            server_contract::TRANSFER_STREAM_PREFACE_LEN,
            server_contract::TRANSFER_STREAM_PREFACE
        )
        == 0
    );
    std::size_t offset = server_contract::TRANSFER_STREAM_PREFACE_LEN;
    std::string archive;
    for (;;) {
        TEST_ASSERT(offset + server_contract::TRANSFER_STREAM_FRAME_HEADER_LEN <= body.size());
        const unsigned char frame_type = static_cast<unsigned char>(body[offset]);
        TEST_ASSERT(body[offset + 1U] == '\0');
        TEST_ASSERT(body[offset + 2U] == '\0');
        TEST_ASSERT(body[offset + 3U] == '\0');
        const std::uint64_t payload_len = read_u64_be(body, offset + 4U);
        offset += server_contract::TRANSFER_STREAM_FRAME_HEADER_LEN;
        TEST_ASSERT(payload_len <= static_cast<std::uint64_t>(body.size() - offset));
        const std::string payload = body.substr(offset, static_cast<std::size_t>(payload_len));
        offset += static_cast<std::size_t>(payload_len);
        if (frame_type == 0x01U) {
            archive += payload;
            continue;
        }
        if (frame_type == 0x02U) {
            TEST_ASSERT(
                Json::parse(payload).at("archive_bytes").get<std::uint64_t>() == archive.size()
            );
            return archive;
        }
        TEST_ASSERT(false);
    }
}

inline std::string encoded_destination_path_header(const test_fs::path& destination) {
    return base64_encode_bytes(destination.string());
}

} // namespace test_tar
