#include <cstdio>
#include <exception>

#include "config.h"
#include "logging.h"
#include "server.h"

int main(int argc, char** argv) {
    init_logging();

    if (argc != 2) {
        std::fprintf(stderr, "usage: %s <daemon-cpp.ini>\n", argv[0]);
        return 2;
    }

    try {
        const DaemonConfig config = load_config(argv[1]);
        log_message(LOG_INFO, "main", "loaded daemon-cpp config for target `" + config.target + "`");
        return run_server(config);
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "main", ex.what());
        std::fprintf(stderr, "%s\n", ex.what());
        return 1;
    }
}
