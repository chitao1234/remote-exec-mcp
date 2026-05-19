#pragma once

#include <string>

struct ParsedPortForwardEndpoint {
    std::string host;
    std::string port;
};

ParsedPortForwardEndpoint parse_port_forward_endpoint(const std::string& endpoint);
unsigned long parse_port_number(const std::string& value);
std::string normalize_port_forward_endpoint(const std::string& endpoint);
std::string ensure_nonzero_connect_endpoint(const std::string& endpoint);
