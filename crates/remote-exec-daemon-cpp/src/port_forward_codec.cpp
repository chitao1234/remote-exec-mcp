#include "port_forward_codec.h"

#include <vector>

#include "port_forward.h"

namespace {

int base64_value(unsigned char ch) {
    if (ch >= 'A' && ch <= 'Z') {
        return static_cast<int>(ch - 'A');
    }
    if (ch >= 'a' && ch <= 'z') {
        return static_cast<int>(ch - 'a') + 26;
    }
    if (ch >= '0' && ch <= '9') {
        return static_cast<int>(ch - '0') + 52;
    }
    if (ch == '+') {
        return 62;
    }
    if (ch == '/') {
        return 63;
    }
    return -1;
}

std::vector<unsigned char> decode_base64_values(const std::string& data) {
    if (data.size() % 4U != 0U) {
        throw PortForwardError(400, "invalid_port_data", "invalid base64 length");
    }

    std::vector<unsigned char> bytes;
    bytes.reserve((data.size() / 4U) * 3U);

    for (std::size_t offset = 0; offset < data.size(); offset += 4U) {
        int values[4];
        int padding = 0;
        for (std::size_t index = 0; index < 4U; ++index) {
            const unsigned char ch = static_cast<unsigned char>(data[offset + index]);
            if (ch == '=') {
                values[index] = 0;
                ++padding;
            } else {
                const int value = base64_value(ch);
                if (value < 0) {
                    throw PortForwardError(400, "invalid_port_data", "invalid base64 data");
                }
                values[index] = value;
            }
        }
        if (padding > 2 || (padding > 0 && offset + 4U != data.size())) {
            throw PortForwardError(400, "invalid_port_data", "invalid base64 padding");
        }
        bytes.push_back(static_cast<unsigned char>((values[0] << 2) | (values[1] >> 4)));
        if (padding < 2) {
            bytes.push_back(
                static_cast<unsigned char>(((values[1] & 0x0f) << 4) | (values[2] >> 2))
            );
        }
        if (padding < 1) {
            bytes.push_back(
                static_cast<unsigned char>(((values[2] & 0x03) << 6) | values[3])
            );
        }
    }

    return bytes;
}

std::string bytes_to_string(const std::vector<unsigned char>& bytes) {
    if (bytes.empty()) {
        return "";
    }
    return std::string(reinterpret_cast<const char*>(bytes.data()), bytes.size());
}

}  // namespace

std::string base64_encode_bytes(const std::string& bytes) {
    static const char alphabet[] =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::string output;
    output.reserve(((bytes.size() + 2U) / 3U) * 4U);
    for (std::size_t offset = 0; offset < bytes.size(); offset += 3U) {
        const unsigned int octet_a = static_cast<unsigned char>(bytes[offset]);
        const unsigned int octet_b =
            offset + 1U < bytes.size() ? static_cast<unsigned char>(bytes[offset + 1U]) : 0U;
        const unsigned int octet_c =
            offset + 2U < bytes.size() ? static_cast<unsigned char>(bytes[offset + 2U]) : 0U;
        const unsigned int triple = (octet_a << 16) | (octet_b << 8) | octet_c;

        output.push_back(alphabet[(triple >> 18) & 0x3f]);
        output.push_back(alphabet[(triple >> 12) & 0x3f]);
        output.push_back(offset + 1U < bytes.size() ? alphabet[(triple >> 6) & 0x3f] : '=');
        output.push_back(offset + 2U < bytes.size() ? alphabet[triple & 0x3f] : '=');
    }
    return output;
}

std::string base64_decode_bytes(const std::string& data) {
    return bytes_to_string(decode_base64_values(data));
}
