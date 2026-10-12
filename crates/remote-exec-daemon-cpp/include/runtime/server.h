#pragma once

#include <atomic>

#include "core/config.h"

int run_server(const DaemonConfig& config);

#if defined(_WIN32) && defined(REMOTE_EXEC_CPP_TESTING)
bool console_shutdown_registered_for_testing();
bool invoke_console_shutdown_handler_for_testing(unsigned long ctrl_type);
#endif
