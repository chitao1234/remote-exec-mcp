#pragma once

#include "platform/socket.h"
#include "runtime/app_context.h"

void handle_client(const HttpConnectionContext& context, UniqueSocket client);
