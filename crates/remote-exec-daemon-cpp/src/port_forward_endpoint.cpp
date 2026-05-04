#include "port_forward_endpoint.h"

#include <cctype>
#include <sstream>

#include "port_forward.h"
#include "text_utils.h"

namespace {

bool all_ascii_digits(const std::string& value) {
    if (value.empty()) {
        return false;
    }
    for (std::size_t i = 0; i < value.size(); ++i) {
        if (!std::isdigit(static_cast<unsigned char>(value[i]))) {
            return false;
        }
    }
    return true;
}

unsigned long endpoint_port(const std::string& endpoint) {
    const ParsedPortForwardEndpoint parsed = parse_port_forward_endpoint(endpoint);
    return parse_port_number(parsed.port);
}

}  // namespace

ParsedPortForwardEndpoint parse_port_forward_endpoint(const std::string& endpoint) {
    ParsedPortForwardEndpoint parsed;
    if (!endpoint.empty() && endpoint[0] == '[') {
        const std::size_t close = endpoint.find(']');
        if (close == std::string::npos) {
            throw PortForwardError(
                400,
                "invalid_endpoint",
                "invalid endpoint `" + endpoint + "`; missing `]`"
            );
        }
        if (close + 1U >= endpoint.size() || endpoint[close + 1U] != ':') {
            throw PortForwardError(
                400,
                "invalid_endpoint",
                "invalid endpoint `" + endpoint + "`; expected [host]:port"
            );
        }
        parsed.host = endpoint.substr(1, close - 1U);
        parsed.port = endpoint.substr(close + 2U);
        return parsed;
    }

    const std::size_t colon = endpoint.rfind(':');
    if (colon == std::string::npos) {
        throw PortForwardError(
            400,
            "invalid_endpoint",
            "invalid endpoint `" + endpoint + "`; expected <port> or <host>:<port>"
        );
    }
    parsed.host = endpoint.substr(0, colon);
    parsed.port = endpoint.substr(colon + 1U);
    return parsed;
}

unsigned long parse_port_number(const std::string& value) {
    if (value.empty()) {
        throw PortForwardError(400, "invalid_endpoint", "invalid port `" + value + "`");
    }
    unsigned long port = 0;
    for (std::size_t i = 0; i < value.size(); ++i) {
        const char ch = value[i];
        if (ch < '0' || ch > '9') {
            throw PortForwardError(400, "invalid_endpoint", "invalid port `" + value + "`");
        }
        const unsigned long digit = static_cast<unsigned long>(ch - '0');
        if (port > (65535UL - digit) / 10UL) {
            throw PortForwardError(400, "invalid_endpoint", "invalid port `" + value + "`");
        }
        port = port * 10UL + digit;
    }
    return port;
}

std::string normalize_port_forward_endpoint(const std::string& endpoint) {
    const std::string trimmed = trim_ascii(endpoint);
    if (trimmed.empty()) {
        throw PortForwardError(400, "invalid_endpoint", "endpoint must not be empty");
    }
    if (all_ascii_digits(trimmed)) {
        const unsigned long port = parse_port_number(trimmed);
        std::ostringstream normalized;
        normalized << "127.0.0.1:" << port;
        return normalized.str();
    }

    const ParsedPortForwardEndpoint parsed = parse_port_forward_endpoint(trimmed);
    if (parsed.host.empty()) {
        throw PortForwardError(400, "invalid_endpoint", "endpoint host must not be empty");
    }
    parse_port_number(parsed.port);
    return trimmed;
}

std::string ensure_nonzero_connect_endpoint(const std::string& endpoint) {
    const std::string normalized = normalize_port_forward_endpoint(endpoint);
    if (endpoint_port(normalized) == 0UL) {
        throw PortForwardError(
            400,
            "invalid_endpoint",
            "connect_endpoint `" + normalized + "` must use a nonzero port"
        );
    }
    return normalized;
}
