# Shared C++ source inventory.
#
# This file intentionally uses make syntax accepted by GNU make, BSD make, and
# NMAKE. Rule logic stays in dialect-specific makefiles.

TRANSFER_SRCS = \
	$(SOURCE_PREFIX)src/transfer_ops.cpp \
	$(SOURCE_PREFIX)src/transfer_ops_fs.cpp \
	$(SOURCE_PREFIX)src/transfer_ops_tar.cpp \
	$(SOURCE_PREFIX)src/transfer_ops_export.cpp \
	$(SOURCE_PREFIX)src/transfer_ops_import.cpp \
	$(SOURCE_PREFIX)src/transfer_glob.cpp

POLICY_SRCS = \
	$(SOURCE_PREFIX)src/path_policy.cpp \
	$(SOURCE_PREFIX)src/path_compare.cpp \
	$(SOURCE_PREFIX)src/filesystem_sandbox.cpp

RPC_FAILURE_SRCS = $(SOURCE_PREFIX)src/rpc_failures.cpp

POSIX_CHILD_REAPER_SRCS = $(SOURCE_PREFIX)src/posix_child_reaper.cpp

ROUTE_SRCS = \
	$(SOURCE_PREFIX)src/server_routes.cpp \
	$(SOURCE_PREFIX)src/server_route_common.cpp \
	$(SOURCE_PREFIX)src/server_route_exec.cpp \
	$(SOURCE_PREFIX)src/server_route_image.cpp \
	$(SOURCE_PREFIX)src/server_route_transfer.cpp \
	$(SOURCE_PREFIX)src/transfer_http_codec.cpp

PORT_FORWARD_SRCS = \
	$(SOURCE_PREFIX)src/port_forward_endpoint.cpp \
	$(SOURCE_PREFIX)src/port_forward_error.cpp \
	$(SOURCE_PREFIX)src/port_forward_socket_ops.cpp \
	$(SOURCE_PREFIX)src/port_tunnel_frame.cpp \
	$(SOURCE_PREFIX)src/port_tunnel.cpp \
	$(SOURCE_PREFIX)src/port_tunnel_sender.cpp \
	$(SOURCE_PREFIX)src/port_tunnel_session.cpp \
	$(SOURCE_PREFIX)src/port_tunnel_spawn.cpp \
	$(SOURCE_PREFIX)src/port_tunnel_streams.cpp \
	$(SOURCE_PREFIX)src/port_tunnel_transport.cpp \
	$(SOURCE_PREFIX)src/port_tunnel_tcp.cpp \
	$(SOURCE_PREFIX)src/port_tunnel_udp.cpp \
	$(SOURCE_PREFIX)src/port_tunnel_error.cpp

BASE64_SRCS = $(SOURCE_PREFIX)src/base64_codec.cpp

PATH_UTILS_SRCS = $(SOURCE_PREFIX)src/path_utils.cpp

SESSION_STORE_SUPPORT_SRCS = \
	$(SOURCE_PREFIX)src/output_renderer.cpp \
	$(SOURCE_PREFIX)src/session_response_builder.cpp

BASE_SRCS_NO_MAIN = \
	$(SOURCE_PREFIX)src/config.cpp \
	$(SOURCE_PREFIX)src/http_codec.cpp \
	$(SOURCE_PREFIX)src/http_connection.cpp \
	$(SOURCE_PREFIX)src/http_helpers.cpp \
	$(SOURCE_PREFIX)src/http_request.cpp \
	$(SOURCE_PREFIX)src/logging.cpp \
	$(SOURCE_PREFIX)src/text_utils.cpp \
	$(SOURCE_PREFIX)src/platform.cpp \
	$(PATH_UTILS_SRCS) \
	$(SOURCE_PREFIX)src/shell_policy.cpp \
	$(SOURCE_PREFIX)src/server.cpp \
	$(SOURCE_PREFIX)src/server_request_utils.cpp \
	$(SOURCE_PREFIX)src/server_runtime.cpp \
	$(SOURCE_PREFIX)src/server_transport.cpp \
	$(SESSION_STORE_SUPPORT_SRCS) \
	$(SOURCE_PREFIX)src/session_store.cpp \
	$(SOURCE_PREFIX)src/session_pump.cpp \
	$(SOURCE_PREFIX)src/patch_engine.cpp \
	$(SOURCE_PREFIX)src/basic_mutex.cpp \
	$(SOURCE_PREFIX)src/connection_manager.cpp \
	$(ROUTE_SRCS) \
	$(PORT_FORWARD_SRCS) \
	$(BASE64_SRCS) \
	$(TRANSFER_SRCS) \
	$(POLICY_SRCS) \
	$(RPC_FAILURE_SRCS)

BASE_SRCS = $(BASE_SRCS_NO_MAIN)

POSIX_SRCS = \
	$(BASE_SRCS) \
	$(SOURCE_PREFIX)src/main.cpp \
	$(SOURCE_PREFIX)src/process_session_posix.cpp \
	$(POSIX_CHILD_REAPER_SRCS)

HOST_PATCH_SRCS = \
	$(SOURCE_PREFIX)tests/test_patch.cpp \
	$(SOURCE_PREFIX)src/patch_engine.cpp \
	$(SOURCE_PREFIX)src/platform.cpp \
	$(PATH_UTILS_SRCS) \
	$(SOURCE_PREFIX)src/path_policy.cpp

HOST_TRANSFER_SRCS = \
	$(SOURCE_PREFIX)tests/test_transfer.cpp \
	$(PATH_UTILS_SRCS) \
	$(TRANSFER_SRCS) \
	$(RPC_FAILURE_SRCS)

HOST_CONFIG_SRCS = \
	$(SOURCE_PREFIX)tests/test_config.cpp \
	$(SOURCE_PREFIX)src/config.cpp \
	$(PATH_UTILS_SRCS) \
	$(SOURCE_PREFIX)src/text_utils.cpp

HOST_BASIC_MUTEX_SRCS = \
	$(SOURCE_PREFIX)tests/test_basic_mutex.cpp \
	$(SOURCE_PREFIX)src/basic_mutex.cpp \
	$(SOURCE_PREFIX)src/platform.cpp

HOST_HTTP_REQUEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_http_request.cpp \
	$(SOURCE_PREFIX)src/http_codec.cpp \
	$(SOURCE_PREFIX)src/http_request.cpp \
	$(SOURCE_PREFIX)src/http_helpers.cpp \
	$(SOURCE_PREFIX)src/text_utils.cpp

HOST_SERVER_TRANSPORT_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_transport.cpp \
	$(SOURCE_PREFIX)src/http_codec.cpp \
	$(SOURCE_PREFIX)src/server_transport.cpp \
	$(SOURCE_PREFIX)src/http_request.cpp \
	$(SOURCE_PREFIX)src/http_helpers.cpp \
	$(SOURCE_PREFIX)src/text_utils.cpp

HOST_SERVER_STREAMING_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_streaming.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_shared.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_routes.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_protocol.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_tcp.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_udp.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_limits.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_lifecycle.cpp \
	$(BASE_SRCS_NO_MAIN) \
	$(SOURCE_PREFIX)src/process_session_posix.cpp \
	$(POSIX_CHILD_REAPER_SRCS)

HOST_SESSION_STORE_SRCS = \
	$(SOURCE_PREFIX)tests/test_session_store.cpp \
	$(SESSION_STORE_SUPPORT_SRCS) \
	$(SOURCE_PREFIX)src/session_store.cpp \
	$(SOURCE_PREFIX)src/session_pump.cpp \
	$(SOURCE_PREFIX)src/process_session_posix.cpp \
	$(POSIX_CHILD_REAPER_SRCS) \
	$(SOURCE_PREFIX)src/platform.cpp \
	$(PATH_UTILS_SRCS) \
	$(SOURCE_PREFIX)src/shell_policy.cpp \
	$(SOURCE_PREFIX)src/basic_mutex.cpp \
	$(SOURCE_PREFIX)src/logging.cpp \
	$(SOURCE_PREFIX)src/config.cpp \
	$(SOURCE_PREFIX)src/text_utils.cpp

HOST_CONNECTION_MANAGER_SRCS = \
	$(SOURCE_PREFIX)tests/test_connection_manager.cpp \
	$(SOURCE_PREFIX)src/connection_manager.cpp \
	$(SOURCE_PREFIX)src/server_transport.cpp \
	$(SOURCE_PREFIX)src/http_codec.cpp \
	$(SOURCE_PREFIX)src/http_request.cpp \
	$(SOURCE_PREFIX)src/http_helpers.cpp \
	$(SOURCE_PREFIX)src/text_utils.cpp \
	$(SOURCE_PREFIX)src/basic_mutex.cpp \
	$(SOURCE_PREFIX)src/platform.cpp \
	$(SOURCE_PREFIX)src/logging.cpp

SERVER_RUNTIME_TEST_SUPPORT_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_runtime.cpp \
	$(BASE_SRCS_NO_MAIN)

SERVER_ROUTES_TEST_SUPPORT_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_routes_shared.cpp \
	$(ROUTE_SRCS) \
	$(SOURCE_PREFIX)src/http_codec.cpp \
	$(SOURCE_PREFIX)src/http_helpers.cpp \
	$(SESSION_STORE_SUPPORT_SRCS) \
	$(SOURCE_PREFIX)src/session_store.cpp \
	$(SOURCE_PREFIX)src/session_pump.cpp \
	$(SOURCE_PREFIX)src/platform.cpp \
	$(PATH_UTILS_SRCS) \
	$(SOURCE_PREFIX)src/shell_policy.cpp \
	$(SOURCE_PREFIX)src/patch_engine.cpp \
	$(SOURCE_PREFIX)src/server_request_utils.cpp \
	$(SOURCE_PREFIX)src/server_transport.cpp \
	$(SOURCE_PREFIX)src/basic_mutex.cpp \
	$(SOURCE_PREFIX)src/logging.cpp \
	$(SOURCE_PREFIX)src/config.cpp \
	$(SOURCE_PREFIX)src/text_utils.cpp \
	$(TRANSFER_SRCS) \
	$(POLICY_SRCS) \
	$(RPC_FAILURE_SRCS) \
	$(PORT_FORWARD_SRCS) \
	$(BASE64_SRCS)

HOST_SERVER_RUNTIME_SRCS = \
	$(SERVER_RUNTIME_TEST_SUPPORT_SRCS) \
	$(SOURCE_PREFIX)src/process_session_posix.cpp \
	$(POSIX_CHILD_REAPER_SRCS)

HOST_SERVER_ROUTES_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_routes.cpp \
	$(SERVER_ROUTES_TEST_SUPPORT_SRCS) \
	$(SOURCE_PREFIX)src/process_session_posix.cpp \
	$(POSIX_CHILD_REAPER_SRCS)

HOST_SERVER_ROUTES_COMMON_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_routes_common.cpp \
	$(SERVER_ROUTES_TEST_SUPPORT_SRCS)

HOST_SANDBOX_SRCS = \
	$(SOURCE_PREFIX)tests/test_sandbox.cpp \
	$(POLICY_SRCS)

HOST_PORT_TUNNEL_FRAME_SRCS = \
	$(SOURCE_PREFIX)tests/test_port_tunnel_frame.cpp \
	$(SOURCE_PREFIX)src/port_tunnel_frame.cpp

WINDOWS_DAEMON_SUPPORT_SRCS = \
	$(SOURCE_PREFIX)src/process_session_win32.cpp \
	$(SOURCE_PREFIX)src/console_output.cpp \
	$(SOURCE_PREFIX)src/win32_error.cpp

WINDOWS_DAEMON_SRCS = \
	$(BASE_SRCS_NO_MAIN) \
	$(SOURCE_PREFIX)src/main.cpp \
	$(WINDOWS_DAEMON_SUPPORT_SRCS)

WINDOWS_SESSION_STORE_TEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_session_store.cpp \
	$(SESSION_STORE_SUPPORT_SRCS) \
	$(SOURCE_PREFIX)src/session_store.cpp \
	$(SOURCE_PREFIX)src/session_pump.cpp \
	$(WINDOWS_DAEMON_SUPPORT_SRCS) \
	$(SOURCE_PREFIX)src/platform.cpp \
	$(PATH_UTILS_SRCS) \
	$(SOURCE_PREFIX)src/shell_policy.cpp \
	$(SOURCE_PREFIX)src/basic_mutex.cpp \
	$(SOURCE_PREFIX)src/logging.cpp \
	$(SOURCE_PREFIX)src/config.cpp \
	$(SOURCE_PREFIX)src/text_utils.cpp

WINDOWS_SERVER_ROUTES_COMMON_TEST_SRCS = \
	$(HOST_SERVER_ROUTES_COMMON_SRCS) \
	$(WINDOWS_DAEMON_SUPPORT_SRCS)

WINDOWS_SERVER_RUNTIME_TEST_SRCS = \
	$(SERVER_RUNTIME_TEST_SUPPORT_SRCS) \
	$(WINDOWS_DAEMON_SUPPORT_SRCS)

# Test source groups by portability. POSIX make currently builds every host
# test; Windows makefiles consume the Windows-capable groups as those tests are
# made portable.
PLATFORM_NEUTRAL_TEST_SRCS = \
	$(HOST_PATCH_SRCS) \
	$(HOST_TRANSFER_SRCS) \
	$(HOST_CONFIG_SRCS) \
	$(HOST_BASIC_MUTEX_SRCS) \
	$(HOST_HTTP_REQUEST_SRCS) \
	$(HOST_SERVER_TRANSPORT_SRCS) \
	$(HOST_CONNECTION_MANAGER_SRCS) \
	$(HOST_SANDBOX_SRCS) \
	$(HOST_PORT_TUNNEL_FRAME_SRCS)

WINDOWS_CAPABLE_PROCESS_TEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_session_store.cpp

WINDOWS_CAPABLE_ROUTE_TEST_SRCS = \
	$(HOST_SERVER_ROUTES_COMMON_SRCS)

WINDOWS_CAPABLE_SERVER_SMOKE_TEST_SRCS = \
	$(SERVER_RUNTIME_TEST_SUPPORT_SRCS)

POSIX_ONLY_TEST_SRCS = \
	$(HOST_SERVER_STREAMING_SRCS) \
	$(HOST_SERVER_ROUTES_SRCS)
