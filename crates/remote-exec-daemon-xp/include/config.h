#pragma once

#include <string>

struct DaemonConfig {
    std::string target;
    std::string listen_host;
    int listen_port;
    std::string default_workdir;
};

DaemonConfig load_config(const std::string& path);
