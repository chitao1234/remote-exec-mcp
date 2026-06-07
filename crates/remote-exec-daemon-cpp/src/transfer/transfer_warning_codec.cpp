#include "transfer_warning_codec.h"
#include "json.hpp"

using Json = nlohmann::json;

namespace transfer_warning_codec {

namespace {

Json transfer_warnings_json(const std::vector<TransferWarning>& warnings) {
    Json json = Json::array();
    for (std::size_t i = 0; i < warnings.size(); ++i) {
        json.push_back(Json{
            {"code", warnings[i].code},
            {"message", warnings[i].message},
        });
    }
    return json;
}

} // namespace

std::string transfer_summary_body(const std::vector<TransferWarning>& warnings) {
    return Json{{"warnings", transfer_warnings_json(warnings)}}.dump();
}

std::vector<TransferWarning> read_transfer_summary(const std::string& body) {
    std::vector<TransferWarning> warnings;
    const Json summary = Json::parse(body);
    const Json raw_warnings = summary.value("warnings", Json::array());
    for (std::size_t i = 0; i < raw_warnings.size(); ++i) {
        warnings.push_back(TransferWarning{
            raw_warnings[i].value("code", std::string()),
            raw_warnings[i].value("message", std::string()),
        });
    }
    return warnings;
}

void append_warnings(
    std::vector<TransferWarning>* destination,
    const std::vector<TransferWarning>& source
) {
    destination->insert(destination->end(), source.begin(), source.end());
}

} // namespace transfer_warning_codec
