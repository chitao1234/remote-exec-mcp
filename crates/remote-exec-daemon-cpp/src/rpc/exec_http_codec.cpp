#include "rpc/exec_http_codec.h"

#include <atomic>
#include <cstdint>
#include <sstream>
#include <string>

#include "../exec/output_renderer.h"
#include "platform/platform.h"

namespace {

std::string make_chunk_id() {
    static std::atomic<unsigned long> next_id(1UL);

    std::ostringstream out;
    out << platform::monotonic_ms() << '-' << next_id.fetch_add(1UL);
    return out.str();
}

double wall_time_seconds(std::uint64_t started_at_ms) {
    const std::uint64_t now = platform::monotonic_ms();
    if (now < started_at_ms) {
        return 0.0;
    }
    return static_cast<double>(now - started_at_ms) / 1000.0;
}

} // namespace

Json exec_session_warnings_json(const std::vector<ExecSessionWarning>& warnings) {
    Json json = Json::array();
    for (std::size_t i = 0; i < warnings.size(); ++i) {
        json.push_back(Json{
            {"code", warnings[i].code},
            {"message", warnings[i].message},
        });
    }
    return json;
}

Json exec_session_result_json(const ExecSessionResult& result, unsigned long max_output_tokens) {
    const std::string trimmed = render_output(result.output, max_output_tokens);
    const unsigned long original_token_count = approximate_output_token_count(result.output.size());
    return Json{
        {"daemon_session_id",
         result.has_daemon_session_id ? Json(result.daemon_session_id) : Json(nullptr)},
        {"running", result.running},
        {"chunk_id", make_chunk_id()},
        {"wall_time_seconds", wall_time_seconds(result.started_at_ms)},
        {"exit_code", result.has_exit_code ? Json(result.exit_code) : Json(nullptr)},
        {"original_token_count", original_token_count},
        {"output", trimmed},
        {"warnings", exec_session_warnings_json(result.warnings)},
    };
}
