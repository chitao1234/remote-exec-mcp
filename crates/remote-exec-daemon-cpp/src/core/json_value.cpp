#include "core/json_value.h"

#include <string>

namespace {

bool depth_guard(int depth, nlohmann::detail::parse_event_t, Json&) {
    if (depth > static_cast<int>(JSON_MAX_DEPTH)) {
        throw JsonRequestError("json nesting depth exceeds limit");
    }
    return true;
}

} // namespace

Json parse_request_json_object(const std::string& text) {
    if (text.empty()) {
        return Json::object();
    }
    try {
        Json parsed = Json::parse(text, &depth_guard);
        if (!parsed.is_object()) {
            throw JsonRequestError("request body must be a JSON object");
        }
        return parsed;
    } catch (const JsonRequestError&) {
        throw;
    } catch (const Json::exception& ex) {
        throw JsonRequestError(ex.what());
    }
}
