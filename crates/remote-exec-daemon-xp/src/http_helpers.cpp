#include <sstream>
#include <stdexcept>

#include "http_helpers.h"

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

void write_json(HttpResponse& res, const Json& body) {
    res.status = 200;
    res.headers["Content-Type"] = "application/json";
    res.body = body.dump();
}

void write_rpc_error(
    HttpResponse& res,
    int status,
    const std::string& code,
    const std::string& message
) {
    res.status = status;
    res.headers["Content-Type"] = "application/json";
    res.body = Json {
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
    case 404:
        return "Not Found";
    case 405:
        return "Method Not Allowed";
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
    headers["Connection"] = "close";
    headers["Content-Length"] = std::to_string(res.body.size());

    for (std::map<std::string, std::string>::const_iterator it = headers.begin();
         it != headers.end();
         ++it) {
        out << it->first << ": " << it->second << "\r\n";
    }

    out << "\r\n";
    out << res.body;
    return out.str();
}
