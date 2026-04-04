#pragma once

#include <map>
#include <string>

#include "json.hpp"

using Json = nlohmann::json;

struct HttpRequest {
    std::string method;
    std::string path;
    std::map<std::string, std::string> headers;
    std::string body;

    std::string header(const std::string& name) const;
};

struct HttpResponse {
    int status;
    std::map<std::string, std::string> headers;
    std::string body;
};

Json parse_json_body(const HttpRequest& req);
void write_json(HttpResponse& res, const Json& body);
void write_rpc_error(
    HttpResponse& res,
    int status,
    const std::string& code,
    const std::string& message
);
std::string render_http_response(const HttpResponse& res);
