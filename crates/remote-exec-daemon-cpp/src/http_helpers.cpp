#include <atomic>
#include <sstream>
#include <stdexcept>

#include "http_helpers.h"

namespace {

bool constant_time_equals(const std::string& actual, const std::string& expected) {
    const std::size_t max_size = actual.size() > expected.size() ? actual.size() : expected.size();
    unsigned int diff = static_cast<unsigned int>(actual.size() ^ expected.size());

    for (std::size_t i = 0; i < max_size; ++i) {
        const unsigned char actual_byte = i < actual.size() ? static_cast<unsigned char>(actual[i]) : 0U;
        const unsigned char expected_byte = i < expected.size() ? static_cast<unsigned char>(expected[i]) : 0U;
        diff |= static_cast<unsigned int>(actual_byte ^ expected_byte);
    }

    return diff == 0U;
}

bool is_log_safe_request_id(const std::string& value) {
    if (value.empty() || value.size() > 128U) {
        return false;
    }
    for (std::string::const_iterator it = value.begin(); it != value.end(); ++it) {
        const unsigned char byte = static_cast<unsigned char>(*it);
        if (byte < 0x21U || byte > 0x7eU) {
            return false;
        }
    }
    return true;
}

std::string generate_request_id() {
    static std::atomic<unsigned long long> next_id(1ULL);
    std::ostringstream out;
    out << "req_cpp_" << std::hex << next_id.fetch_add(1ULL);
    return out.str();
}

} // namespace

std::string HttpRequest::header(const std::string& name) const {
    std::map<std::string, std::string>::const_iterator it = headers.find(name);
    if (it == headers.end()) {
        return "";
    }
    return it->second;
}

Json parse_json_body(const HttpRequest& req) {
    if (req.body.empty()) {
        return Json::object();
    }
    return Json::parse(req.body);
}

bool request_has_bearer_auth(const HttpRequest& req, const std::string& bearer_token) {
    const std::string expected = "Bearer " + bearer_token;
    return constant_time_equals(req.header("authorization"), expected);
}

const char* request_id_header_name() {
    return "x-request-id";
}

std::string request_id_for_request(const HttpRequest& req) {
    if (is_log_safe_request_id(req.request_id)) {
        return req.request_id;
    }

    const std::string header_value = req.header(request_id_header_name());
    if (is_log_safe_request_id(header_value)) {
        return header_value;
    }

    return generate_request_id();
}

void assign_request_id(HttpRequest& req) {
    req.request_id = request_id_for_request(req);
}

void write_request_id_header(HttpResponse& res, const HttpRequest& req) {
    res.headers[request_id_header_name()] = request_id_for_request(req);
}

void write_json(HttpResponse& res, const Json& body) {
    res.status = 200;
    res.headers["Content-Type"] = "application/json";
    res.body = body.dump();
}

void write_bearer_auth_challenge(HttpResponse& res) {
    write_rpc_error(res, 401, "unauthorized", "missing or invalid bearer token");
    res.headers["WWW-Authenticate"] = "Bearer";
}

void write_rpc_error(HttpResponse& res, int status, const std::string& code, const std::string& message) {
    res.status = status;
    res.headers["Content-Type"] = "application/json";
    res.body =
        Json{
            {"code", code},
            {"message", message},
        }
            .dump();
}

static std::string reason_phrase(int status) {
    switch (status) {
    case 200:
        return "OK";
    case 400:
        return "Bad Request";
    case 401:
        return "Unauthorized";
    case 404:
        return "Not Found";
    case 405:
        return "Method Not Allowed";
    case 413:
        return "Payload Too Large";
    case 429:
        return "Too Many Requests";
    case 500:
        return "Internal Server Error";
    default:
        return "Error";
    }
}

std::string render_http_response(const HttpResponse& res) {
    std::ostringstream out;
    out << "HTTP/1.1 " << res.status << ' ' << reason_phrase(res.status) << "\r\n";

    std::map<std::string, std::string> headers = res.headers;
    headers["Content-Length"] = std::to_string(res.body.size());

    for (std::map<std::string, std::string>::const_iterator it = headers.begin(); it != headers.end(); ++it) {
        out << it->first << ": " << it->second << "\r\n";
    }

    out << "\r\n";
    out << res.body;
    return out.str();
}

std::string render_http_upgrade_response(const std::string& upgrade_token,
                                         const std::map<std::string, std::string>& headers) {
    std::ostringstream out;
    out << "HTTP/1.1 101 Switching Protocols\r\n";
    out << "Connection: Upgrade\r\n";
    out << "Upgrade: " << upgrade_token << "\r\n";

    for (std::map<std::string, std::string>::const_iterator it = headers.begin(); it != headers.end(); ++it) {
        out << it->first << ": " << it->second << "\r\n";
    }

    out << "\r\n";
    return out.str();
}
