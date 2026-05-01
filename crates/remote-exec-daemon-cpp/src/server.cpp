#include <cstdint>
#include <memory>
#include <sstream>
#include <string>

#ifdef _WIN32
#include <winsock2.h>
#include <windows.h>
#else
#include <thread>
#endif

#include "common.h"
#include "http_request.h"
#include "http_helpers.h"
#include "logging.h"
#include "platform.h"
#include "server.h"
#include "server_routes.h"
#include "server_transport.h"

namespace {

std::string daemon_instance_id() {
    std::ostringstream out;
    out << platform::monotonic_ms();
    return out.str();
}

void handle_client(AppState& state, UniqueSocket client) {
    try {
        const std::uint64_t started_at_ms = platform::monotonic_ms();
        const std::string raw_request = read_http_request(
            client.get(),
            state.config.max_request_header_bytes,
            state.config.max_request_body_bytes
        );
        const HttpRequest request = parse_http_request(raw_request);
        const HttpResponse response = route_request(state, request);
        {
            std::ostringstream message;
            message << request.method << ' ' << request.path
                    << " status=" << response.status
                    << " elapsed_ms=" << (platform::monotonic_ms() - started_at_ms);
            log_message(level_for_status(response.status), "server", message.str());
        }
        send_all(client.get(), render_http_response(response));
    } catch (const BadHttpRequest& ex) {
        log_message(LOG_WARN, "server", ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, "bad_request", ex.what());
        send_all(client.get(), render_http_response(response));
    } catch (const HttpParseError& ex) {
        log_message(LOG_WARN, "server", ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, "bad_request", ex.what());
        send_all(client.get(), render_http_response(response));
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", ex.what());
        HttpResponse response;
        response.status = 500;
        write_rpc_error(response, 500, "internal_error", ex.what());
        send_all(client.get(), render_http_response(response));
    }
}

#ifdef _WIN32
struct ClientThreadContext {
    AppState* state;
    SOCKET socket;
};

DWORD WINAPI client_thread_entry(LPVOID raw_context) {
    std::unique_ptr<ClientThreadContext> context(
        static_cast<ClientThreadContext*>(raw_context)
    );
    UniqueSocket client(context->socket);
    handle_client(*context->state, std::move(client));
    return 0;
}
#endif

void spawn_client_thread(AppState& state, UniqueSocket client) {
#ifdef _WIN32
    std::unique_ptr<ClientThreadContext> context(new ClientThreadContext());
    context->state = &state;
    context->socket = client.release();
    HANDLE handle = CreateThread(NULL, 0, client_thread_entry, context.get(), 0, NULL);
    if (handle == NULL) {
        UniqueSocket cleanup(context->socket);
        log_message(LOG_ERROR, "server", "CreateThread failed");
        return;
    }
    context.release();
    CloseHandle(handle);
#else
    std::thread(
        [&state](SOCKET socket) {
            UniqueSocket thread_client(socket);
            handle_client(state, std::move(thread_client));
        },
        client.release()
    )
        .detach();
#endif
}

}  // namespace

int run_server(const DaemonConfig& config) {
    NetworkSession network;

    AppState state;
    state.config = config;
    state.daemon_instance_id = daemon_instance_id();
    state.hostname = platform::hostname();
    state.default_shell = platform::resolve_default_shell(config.default_shell);
    UniqueSocket listener(create_listener(state.config));

    {
        std::ostringstream message;
        message << "listening on " << state.config.listen_host << ':'
                << state.config.listen_port
                << " target=`" << state.config.target << "`"
                << " http_auth_enabled=`"
                << (!state.config.http_auth_bearer_token.empty() ? "true" : "false") << "`"
                << " platform=`" << platform::platform_name() << "`"
                << " arch=`" << platform::arch_name() << "`"
                << " default_shell=`" << state.default_shell << "`"
                << " daemon_instance_id=`" << state.daemon_instance_id << "`";
        log_message(LOG_INFO, "server", message.str());
    }

    for (;;) {
        UniqueSocket client(accept_client(listener.get()));
        if (!client.valid()) {
            log_message(LOG_WARN, "server", "accept failed");
            continue;
        }

        spawn_client_thread(state, std::move(client));
    }

    return 0;
}
