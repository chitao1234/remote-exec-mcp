#include <sstream>
#include <string>

#include "logging.h"
#include "platform.h"
#ifndef _WIN32
#include "posix_child_reaper.h"
#endif
#include "server.h"
#include "server_runtime.h"

int run_server(const DaemonConfig& config) {
    NetworkSession network;
    ServerRuntime runtime(config);
#ifndef _WIN32
    install_posix_child_reaper();
#endif
    runtime.start_accept_loop();

    {
        std::ostringstream message;
        message << "listening on " << runtime.state().config.listen_host << ':'
                << runtime.bound_port()
                << " target=`" << runtime.state().config.target << "`"
                << " http_auth_enabled=`"
                << (!runtime.state().config.http_auth_bearer_token.empty() ? "true" : "false")
                << "`"
                << " platform=`" << platform::platform_name() << "`"
                << " arch=`" << platform::arch_name() << "`"
                << " default_shell=`" << runtime.state().default_shell << "`"
                << " daemon_instance_id=`" << runtime.state().daemon_instance_id << "`";
        log_message(LOG_INFO, "server", message.str());
    }

    runtime.join();
    return 0;
}
