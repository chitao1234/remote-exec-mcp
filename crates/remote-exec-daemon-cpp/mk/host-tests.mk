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
	SANDBOX \
	PORT_TUNNEL_FRAME \
	REACTOR_CORE

HOST_BASIC_MUTEX_BIN = test_basic_mutex
HOST_PATCH_BIN = test_patch
HOST_TRANSFER_BIN = test_transfer
HOST_CONFIG_BIN = test_config
HOST_HTTP_REQUEST_BIN = test_http_request
HOST_SANDBOX_BIN = test_sandbox
HOST_PORT_TUNNEL_FRAME_BIN = test_port_tunnel_frame
HOST_REACTOR_CORE_BIN = test_reactor_core

HOST_BASIC_MUTEX_NAME = basic-mutex
HOST_PATCH_NAME = patch
HOST_TRANSFER_NAME = transfer
HOST_CONFIG_NAME = config
HOST_HTTP_REQUEST_NAME = http-request
HOST_SANDBOX_NAME = sandbox
HOST_PORT_TUNNEL_FRAME_NAME = port-tunnel-frame
HOST_REACTOR_CORE_NAME = reactor-core

HOST_BASIC_MUTEX_TEST_TARGET = test-host-basic-mutex
HOST_PATCH_TEST_TARGET = test-host-patch
HOST_TRANSFER_TEST_TARGET = test-host-transfer
HOST_CONFIG_TEST_TARGET = test-host-config
HOST_HTTP_REQUEST_TEST_TARGET = test-host-http-request
HOST_SANDBOX_TEST_TARGET = test-host-sandbox
HOST_PORT_TUNNEL_FRAME_TEST_TARGET = test-port-tunnel-frame
HOST_REACTOR_CORE_TEST_TARGET = test-host-reactor-core
