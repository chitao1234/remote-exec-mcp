#include <string>

#include "logging.h"
#include "server_route_port_forward.h"

HttpResponse handle_port_listen(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const Json lease = body.value("lease", Json());
        write_json(
            response,
            state.port_forwards.listen(
                body.at("endpoint").get<std::string>(),
                body.at("protocol").get<std::string>(),
                lease.is_null() ? std::string() : lease.at("lease_id").get<std::string>(),
                lease.is_null() ? 0U : lease.at("ttl_ms").get<std::uint64_t>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/listen failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(LOG_WARN, "server", std::string("port/listen bad request: ") + ex.what());
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/listen failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_listen_accept(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.listen_accept(body.at("bind_id").get<std::string>())
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/listen/accept failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/listen/accept bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/listen/accept failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_listen_close(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.listen_close(body.at("bind_id").get<std::string>())
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/listen/close failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/listen/close bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/listen/close failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_lease_renew(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.lease_renew(
                body.at("lease_id").get<std::string>(),
                body.at("ttl_ms").get<std::uint64_t>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/lease/renew failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/lease/renew bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/lease/renew failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_connect(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const Json lease = body.value("lease", Json());
        write_json(
            response,
            state.port_forwards.connect(
                body.at("endpoint").get<std::string>(),
                body.at("protocol").get<std::string>(),
                lease.is_null() ? std::string() : lease.at("lease_id").get<std::string>(),
                lease.is_null() ? 0U : lease.at("ttl_ms").get<std::uint64_t>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/connect failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(LOG_WARN, "server", std::string("port/connect bad request: ") + ex.what());
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/connect failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_connection_read(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.connection_read(
                body.at("connection_id").get<std::string>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/connection/read failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/connection/read bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(
            LOG_ERROR,
            "server",
            std::string("port/connection/read failed: ") + ex.what()
        );
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_connection_write(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.connection_write(
                body.at("connection_id").get<std::string>(),
                body.at("data").get<std::string>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/connection/write failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/connection/write bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(
            LOG_ERROR,
            "server",
            std::string("port/connection/write failed: ") + ex.what()
        );
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_connection_close(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.connection_close(
                body.at("connection_id").get<std::string>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/connection/close failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/connection/close bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(
            LOG_ERROR,
            "server",
            std::string("port/connection/close failed: ") + ex.what()
        );
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_udp_read(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.udp_datagram_read(body.at("bind_id").get<std::string>())
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/udp/read failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(LOG_WARN, "server", std::string("port/udp/read bad request: ") + ex.what());
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/udp/read failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_udp_write(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.udp_datagram_write(
                body.at("bind_id").get<std::string>(),
                body.at("peer").get<std::string>(),
                body.at("data").get<std::string>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/udp/write failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(LOG_WARN, "server", std::string("port/udp/write bad request: ") + ex.what());
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/udp/write failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}
