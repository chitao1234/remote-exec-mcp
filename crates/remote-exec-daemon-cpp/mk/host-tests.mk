# Shared host-native POSIX test inventory.
#
# Keep this file to plain variable assignments so GNU make and BSD make can
# both consume it while keeping dialect-specific rule generation separate.

HOST_POSIX_TESTS = \
	BASIC_MUTEX \
	PATCH \
	TRANSFER \
	CONFIG \
	HTTP_REQUEST \
	SERVER_TRANSPORT \
	SERVER_STREAMING \
	SESSION_STORE \
	CONNECTION_MANAGER \
	SERVER_RUNTIME \
	SERVER_ROUTES \
	SANDBOX \
	PORT_TUNNEL_FRAME

HOST_BASIC_MUTEX_BIN = test_basic_mutex
HOST_PATCH_BIN = test_patch
HOST_TRANSFER_BIN = test_transfer
HOST_CONFIG_BIN = test_config
HOST_HTTP_REQUEST_BIN = test_http_request
HOST_SERVER_TRANSPORT_BIN = test_server_transport
HOST_SERVER_STREAMING_BIN = test_server_streaming
HOST_SESSION_STORE_BIN = test_session_store
HOST_CONNECTION_MANAGER_BIN = test_connection_manager
HOST_SERVER_RUNTIME_BIN = test_server_runtime
HOST_SERVER_ROUTES_BIN = test_server_routes
HOST_SANDBOX_BIN = test_sandbox
HOST_PORT_TUNNEL_FRAME_BIN = test_port_tunnel_frame

HOST_BASIC_MUTEX_TEST_TARGET = test-host-basic-mutex
HOST_PATCH_TEST_TARGET = test-host-patch
HOST_TRANSFER_TEST_TARGET = test-host-transfer
HOST_CONFIG_TEST_TARGET = test-host-config
HOST_HTTP_REQUEST_TEST_TARGET = test-host-http-request
HOST_SERVER_TRANSPORT_TEST_TARGET = test-host-server-transport
HOST_SERVER_STREAMING_TEST_TARGET = test-host-server-streaming
HOST_SESSION_STORE_TEST_TARGET = test-host-session-store
HOST_CONNECTION_MANAGER_TEST_TARGET = test-host-connection-manager
HOST_SERVER_RUNTIME_TEST_TARGET = test-host-server-runtime
HOST_SERVER_ROUTES_TEST_TARGET = test-host-server-routes
HOST_SANDBOX_TEST_TARGET = test-host-sandbox
HOST_PORT_TUNNEL_FRAME_TEST_TARGET = test-port-tunnel-frame
