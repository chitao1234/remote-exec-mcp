#include "session_response_builder.h"

#include <atomic>
#include <sstream>

#include "output_renderer.h"
#include "platform.h"

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

Json build_session_response(const char* daemon_session_id,
                            bool running,
                            std::uint64_t started_at_ms,
                            bool has_exit_code,
                            int exit_code,
                            const std::string& output,
                            unsigned long max_output_tokens,
                            const Json& warnings) {
    const std::string trimmed = render_output(output, max_output_tokens);
    const unsigned long original_token_count = approximate_output_token_count(output.size());
    return Json{{"daemon_session_id", daemon_session_id != nullptr ? Json(daemon_session_id) : Json(nullptr)},
                {"running", running},
                {"chunk_id", make_chunk_id()},
                {"wall_time_seconds", wall_time_seconds(started_at_ms)},
                {"exit_code", has_exit_code ? Json(exit_code) : Json(nullptr)},
                {"original_token_count", original_token_count},
                {"output", trimmed},
                {"warnings", warnings}};
}
