#include <cctype>
#include <limits>
#include <map>
#include <string>

#include "http_codec.h"
#include "text_utils.h"

namespace {

std::size_t parse_content_length_value(const std::string& raw) {
    if (raw.empty()) {
        throw HttpProtocolError("invalid Content-Length");
    }

    std::size_t value = 0;
    for (std::size_t i = 0; i < raw.size(); ++i) {
        const char ch = raw[i];
        if (ch < '0' || ch > '9') {
            throw HttpProtocolError("invalid Content-Length");
        }
        const std::size_t digit = static_cast<std::size_t>(ch - '0');
        if (value > (std::numeric_limits<std::size_t>::max() - digit) / 10U) {
            throw HttpProtocolError("Content-Length is too large");
        }
        value = value * 10U + digit;
    }
    return value;
}

int hex_digit_value(char ch) {
    if (ch >= '0' && ch <= '9') {
        return ch - '0';
    }
    if (ch >= 'a' && ch <= 'f') {
        return ch - 'a' + 10;
    }
    if (ch >= 'A' && ch <= 'F') {
        return ch - 'A' + 10;
    }
    return -1;
}

bool is_valid_header_value(const std::string& value) {
    for (std::size_t i = 0; i < value.size(); ++i) {
        const unsigned char ch = static_cast<unsigned char>(value[i]);
        if ((ch < 32U && ch != '\t') || ch == 127U) {
            return false;
        }
    }
    return true;
}

void validate_header_name(const std::string& raw_name) {
    if (raw_name.empty() || trim_ascii(raw_name) != raw_name) {
        throw HttpProtocolError("invalid header name");
    }

    for (std::size_t i = 0; i < raw_name.size(); ++i) {
        if (!is_http_token_char(raw_name[i])) {
            throw HttpProtocolError("invalid header name");
        }
    }
}

} // namespace

HttpRequestBodyFraming::HttpRequestBodyFraming() : has_content_length(false), content_length(0), chunked(false) {
}

void parse_http_header_line(const std::string& header_line, std::map<std::string, std::string>* headers) {
    const std::size_t colon = header_line.find(':');
    if (colon == std::string::npos) {
        throw HttpProtocolError("invalid header line");
    }

    const std::string raw_name = header_line.substr(0, colon);
    validate_header_name(raw_name);

    const std::string name = lowercase_ascii(raw_name);
    if (headers->find(name) != headers->end()) {
        throw HttpProtocolError("duplicate header");
    }

    const std::string value = trim_ascii(header_line.substr(colon + 1));
    if (!is_valid_header_value(value)) {
        throw HttpProtocolError("invalid header value");
    }
    (*headers)[name] = value;
}

HttpRequestBodyFraming request_body_framing_from_headers(const std::map<std::string, std::string>& headers) {
    HttpRequestBodyFraming framing;
    const std::map<std::string, std::string>::const_iterator content_length = headers.find("content-length");
    if (content_length != headers.end()) {
        framing.has_content_length = true;
        framing.content_length = parse_content_length_value(content_length->second);
    }

    const std::map<std::string, std::string>::const_iterator transfer_encoding = headers.find("transfer-encoding");
    if (transfer_encoding != headers.end()) {
        if (lowercase_ascii(transfer_encoding->second) != "chunked") {
            throw HttpProtocolError("unsupported Transfer-Encoding");
        }
        framing.chunked = true;
    }

    if (framing.chunked && framing.has_content_length) {
        throw HttpProtocolError("chunked request cannot include Content-Length");
    }

    return framing;
}

std::size_t parse_http_chunk_size_line(const std::string& line) {
    const std::size_t extension = line.find(';');
    const std::string size_text = trim_ascii(extension == std::string::npos ? line : line.substr(0, extension));
    if (size_text.empty()) {
        throw HttpProtocolError("invalid chunk size");
    }

    std::size_t value = 0;
    for (std::size_t i = 0; i < size_text.size(); ++i) {
        const int digit = hex_digit_value(size_text[i]);
        if (digit < 0) {
            throw HttpProtocolError("invalid chunk size");
        }
        const std::size_t chunk_digit = static_cast<std::size_t>(digit);
        if (value > (std::numeric_limits<std::size_t>::max() - chunk_digit) / 16U) {
            throw HttpProtocolError("chunk size is too large");
        }
        value = value * 16U + chunk_digit;
    }
    return value;
}

std::string decode_http_chunked_body(const std::string& body) {
    std::string decoded;
    std::size_t offset = 0;
    std::map<std::string, std::string> trailers;

    for (;;) {
        const std::size_t line_end = body.find("\r\n", offset);
        if (line_end == std::string::npos) {
            throw HttpProtocolError("incomplete chunked body");
        }

        const std::size_t chunk_size = parse_http_chunk_size_line(body.substr(offset, line_end - offset));
        offset = line_end + 2U;

        if (chunk_size == 0U) {
            for (;;) {
                const std::size_t trailer_line_end = body.find("\r\n", offset);
                if (trailer_line_end == std::string::npos) {
                    throw HttpProtocolError("incomplete chunked body");
                }
                if (trailer_line_end == offset) {
                    offset += 2U;
                    if (offset != body.size()) {
                        throw HttpProtocolError("extra data after chunked body");
                    }
                    return decoded;
                }

                parse_http_header_line(body.substr(offset, trailer_line_end - offset), &trailers);
                offset = trailer_line_end + 2U;
            }
        }

        if (chunk_size > body.size() - offset) {
            throw HttpProtocolError("incomplete chunked body");
        }
        const std::size_t chunk_end = offset + chunk_size;
        if (body.size() - chunk_end < 2U) {
            throw HttpProtocolError("incomplete chunked body");
        }
        if (body.compare(chunk_end, 2U, "\r\n") != 0) {
            throw HttpProtocolError("invalid chunked body");
        }

        decoded.append(body, offset, chunk_size);
        offset = chunk_end + 2U;
    }
}
