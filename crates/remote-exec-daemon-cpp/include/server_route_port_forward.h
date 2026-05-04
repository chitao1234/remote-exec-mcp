#pragma once

#include "http_helpers.h"
#include "server.h"

HttpResponse handle_port_listen(AppState& state, const HttpRequest& request);
HttpResponse handle_port_listen_accept(AppState& state, const HttpRequest& request);
HttpResponse handle_port_listen_close(AppState& state, const HttpRequest& request);
HttpResponse handle_port_connect(AppState& state, const HttpRequest& request);
HttpResponse handle_port_connection_read(AppState& state, const HttpRequest& request);
HttpResponse handle_port_connection_write(AppState& state, const HttpRequest& request);
HttpResponse handle_port_connection_close(AppState& state, const HttpRequest& request);
HttpResponse handle_port_udp_read(AppState& state, const HttpRequest& request);
HttpResponse handle_port_udp_write(AppState& state, const HttpRequest& request);
