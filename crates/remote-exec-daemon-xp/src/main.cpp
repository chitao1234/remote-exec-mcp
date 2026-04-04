#include <cstdio>
#include <exception>

#include "config.h"
#include "server.h"

int main(int argc, char** argv) {
    if (argc != 2) {
        std::fprintf(stderr, "usage: %s <daemon-xp.ini>\n", argv[0]);
        return 2;
    }

    try {
        const DaemonConfig config = load_config(argv[1]);
        return run_server(config);
    } catch (const std::exception& ex) {
        std::fprintf(stderr, "%s\n", ex.what());
        return 1;
    }
}
