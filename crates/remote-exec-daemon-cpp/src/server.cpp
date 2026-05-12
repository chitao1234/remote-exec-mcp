#include <fstream>
#include <sstream>
#include <stdexcept>
#include <string>

#include "logging.h"
#include "platform.h"
#ifndef _WIN32
#include "posix_child_reaper.h"
#endif
#include "server.h"
#include "server_runtime.h"

static void write_test_bound_addr_file(const DaemonConfig& config, unsigned short bound_port) {
    if (config.test_bound_addr_file.empty()) {
        return;
    }
    std::ofstream out(config.test_bound_addr_file.c_str(), std::ios::out | std::ios::trunc);
    if (!out) {
        throw std::runtime_error("failed to open test_bound_addr_file");
    }
    out << config.listen_host << ':' << bound_port << '\n';
    if (!out) {
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

    {
        std::ostringstream message;
        message << "listening on " << runtime.state().config.listen_host << ':' << bound_port << " target=`"
                << runtime.state().config.target << "`" << " http_auth_enabled=`"
                << (!runtime.state().config.http_auth_bearer_token.empty() ? "true" : "false") << "`" << " platform=`"
                << platform::platform_name() << "`" << " arch=`" << platform::arch_name() << "`" << " default_shell=`"
                << runtime.state().default_shell << "`" << " daemon_instance_id=`" << runtime.state().daemon_instance_id
                << "`";
        log_message(LOG_INFO, "server", message.str());
    }

    runtime.join();
    return 0;
}
