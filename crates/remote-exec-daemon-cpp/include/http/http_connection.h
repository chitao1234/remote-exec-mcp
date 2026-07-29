#pragma once

#include <functional>
#include <memory>

#include "http/connection_transport.h"
#include "runtime/route_context.h"

void handle_client(
    const HttpConnectionContext& context,
    const std::shared_ptr<ConnectionTransport>& client,
    const std::function<void()>& on_first_request = std::function<void()>()
);
void handle_client(
    const HttpConnectionContext& context,
    UniqueSocket client,
    const std::function<void()>& on_first_request = std::function<void()>()
);
