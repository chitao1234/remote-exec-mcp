#include <cstdio>
#include <stdexcept>
#include <string>

#include "logging.h"
#include "path_utils.h"
#include "platform.h"
#ifndef _WIN32
#include "posix_child_reaper.h"
#endif
#include "scoped_file.h"
#include "server.h"
#include "server_runtime.h"

static void write_test_bound_addr_file(const DaemonConfig& config, unsigned short bound_port) {
    if (config.test_bound_addr_file.empty()) {
        return;
    }
    ScopedFile out(path_utils::open_file(config.test_bound_addr_file, "wb"));
    if (!out.valid()) {
        throw std::runtime_error("failed to open test_bound_addr_file");
    }
    const std::string line = config.listen_host + ":" + std::to_string(bound_port) + "\n";
    if (std::fwrite(line.data(), 1, line.size(), out.get()) != line.size() || out.close() != 0) {
        throw std::runtime_error("failed to write test_bound_addr_file");
    }
}

int run_server(const DaemonConfig& config) {
    NetworkSession network;
    ServerRuntime runtime(config);
#ifndef _WIN32
    install_posix_child_reaper();
#endif
    runtime.start_accept_loop();
    const unsigned short bound_port = runtime.bound_port();
    write_test_bound_addr_file(runtime.state().config, bound_port);

    LogMessageBuilder message("listening");
    message.raw("on")
        .raw(runtime.state().config.listen_host)
        .field("port", bound_port)
        .quoted_field("target", runtime.state().config.target)
        .bool_field("http_auth_enabled", !runtime.state().config.http_auth_bearer_token.empty())
        .quoted_field("platform", platform::platform_name())
        .quoted_field("arch", platform::arch_name())
        .quoted_field("default_shell", runtime.state().default_shell)
        .quoted_field("daemon_instance_id", runtime.state().daemon_instance_id);
    log_message(LOG_INFO, "server", message.str());

    runtime.join();
    return 0;
}
