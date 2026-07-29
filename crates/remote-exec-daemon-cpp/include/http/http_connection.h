#pragma once

#include <functional>

#include "platform/socket.h"
#include "runtime/route_context.h"

void handle_client(
    const HttpConnectionContext& context,
    UniqueSocket client,
    const std::function<void()>& on_first_request = std::function<void()>()
);
