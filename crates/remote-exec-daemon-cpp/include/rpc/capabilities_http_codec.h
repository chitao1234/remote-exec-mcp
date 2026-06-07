#pragma once

#include "capabilities/daemon_capabilities.h"
#include "http/http_helpers.h"

void write_daemon_capabilities(Json* target, const DaemonCapabilities& capabilities);
