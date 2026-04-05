#pragma once

#include <string>

#include <winsock2.h>

#include "config.h"

std::string read_http_request(SOCKET client);
void send_all(SOCKET client, const std::string& data);
SOCKET create_listener(const DaemonConfig& config);
