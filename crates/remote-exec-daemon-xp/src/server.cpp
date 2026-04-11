#include <sstream>
#include <string>

#include "common.h"
#include "http_request.h"
#include "http_helpers.h"
#include "logging.h"
#include "server.h"
#include "server_routes.h"
#include "server_transport.h"
#include "win32_scoped.h"

namespace {

std::string daemon_instance_id() {
    std::ostringstream out;
    out << GetTickCount();
    return out.str();
}

std::string hostname_string() {
    char buffer[MAX_COMPUTERNAME_LENGTH + 1];
    DWORD size = MAX_COMPUTERNAME_LENGTH + 1;
    if (GetComputerNameA(buffer, &size) == 0) {
        return "windows-xp";
    }
    return std::string(buffer, size);
}

}  // namespace

int run_server(const DaemonConfig& config) {
    WinsockSession winsock;

    AppState state;
    state.config = config;
    state.daemon_instance_id = daemon_instance_id();
    state.hostname = hostname_string();
    UniqueSocket listener(create_listener(state.config));

    {
        std::ostringstream message;
        message << "listening on " << state.config.listen_host << ':'
                << state.config.listen_port
                << " target=`" << state.config.target << "`"
                << " http_auth_enabled=`"
                << (!state.config.http_auth_bearer_token.empty() ? "true" : "false") << "`"
                << " daemon_instance_id=`" << state.daemon_instance_id << "`";
        log_message(LOG_INFO, "server", message.str());
    }

    for (;;) {
        UniqueSocket client(accept(listener.get(), NULL, NULL));
        if (!client.valid()) {
            log_message(LOG_WARN, "server", "accept failed");
            continue;
        }

        try {
            const DWORD started_at_ms = GetTickCount();
            const std::string raw_request = read_http_request(client.get());
            const HttpRequest request = parse_http_request(raw_request);
            const HttpResponse response = route_request(state, request);
            {
                std::ostringstream message;
                message << request.method << ' ' << request.path
                        << " status=" << response.status
                        << " elapsed_ms=" << (GetTickCount() - started_at_ms);
                log_message(level_for_status(response.status), "server", message.str());
            }
            send_all(client.get(), render_http_response(response));
        } catch (const std::exception& ex) {
            log_message(LOG_ERROR, "server", ex.what());
            HttpResponse response;
            response.status = 500;
            write_rpc_error(response, 500, "internal_error", ex.what());
            send_all(client.get(), render_http_response(response));
        }
    }

    return 0;
}
