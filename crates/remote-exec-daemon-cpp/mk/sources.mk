# Shared C++ source inventory.
#
# This file intentionally uses make syntax accepted by GNU make, BSD make, and
# NMAKE. Rule logic stays in dialect-specific makefiles.

TRANSFER_SRCS = \
	$(SOURCE_PREFIX)src/transfer/transfer_ops.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_ops_fs.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_ops_tar.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_ops_export.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_ops_import.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_glob.cpp

POLICY_SRCS = \
	$(SOURCE_PREFIX)src/policy/path_policy.cpp \
	$(SOURCE_PREFIX)src/policy/path_compare.cpp \
	$(SOURCE_PREFIX)src/policy/filesystem_sandbox.cpp

RPC_FAILURE_SRCS = $(SOURCE_PREFIX)src/rpc/rpc_failures.cpp

PATCH_SRCS = $(SOURCE_PREFIX)src/patch/patch_engine.cpp

IMAGE_SRCS = $(SOURCE_PREFIX)src/image/image_ops.cpp

CONFIG_SRCS = $(SOURCE_PREFIX)src/core/config.cpp

LOGGING_SRCS = $(SOURCE_PREFIX)src/core/logging.cpp

TEXT_UTILS_SRCS = $(SOURCE_PREFIX)src/core/text_utils.cpp

SHELL_POLICY_SRCS = $(SOURCE_PREFIX)src/core/shell_policy.cpp

CAPABILITIES_SRCS = $(SOURCE_PREFIX)src/capabilities/daemon_capabilities.cpp

POSIX_CHILD_REAPER_SRCS = $(SOURCE_PREFIX)src/exec/posix_child_reaper.cpp

POSIX_PROCESS_SESSION_SRCS = \
	$(SOURCE_PREFIX)src/exec/process_session_posix.cpp \
	$(POSIX_CHILD_REAPER_SRCS)

DAEMON_THREAD_SRCS = $(SOURCE_PREFIX)src/runtime/daemon_thread.cpp

RUNTIME_SRCS = \
	$(SOURCE_PREFIX)src/runtime/server.cpp \
	$(SOURCE_PREFIX)src/runtime/server_runtime.cpp \
	$(SOURCE_PREFIX)src/runtime/connection_manager.cpp \
	$(DAEMON_THREAD_SRCS)

PLATFORM_SRCS = $(SOURCE_PREFIX)src/platform/platform.cpp

HTTP_SRCS = \
	$(SOURCE_PREFIX)src/http/http_codec.cpp \
	$(SOURCE_PREFIX)src/http/http_connection.cpp \
	$(SOURCE_PREFIX)src/http/http_helpers.cpp \
	$(SOURCE_PREFIX)src/http/http_request.cpp

ROUTE_SRCS = \
	$(SOURCE_PREFIX)src/rpc/exec_request_utils.cpp \
	$(IMAGE_SRCS) \
	$(SOURCE_PREFIX)src/rpc/server_contract.cpp \
	$(SOURCE_PREFIX)src/rpc/server_request_utils.cpp \
	$(SOURCE_PREFIX)src/rpc/server_routes.cpp \
	$(SOURCE_PREFIX)src/rpc/server_route_common.cpp \
	$(SOURCE_PREFIX)src/rpc/server_route_exec.cpp \
	$(SOURCE_PREFIX)src/rpc/server_route_image.cpp \
	$(SOURCE_PREFIX)src/rpc/server_route_transfer.cpp \
	$(SOURCE_PREFIX)src/rpc/transfer_http_codec.cpp \
	$(SOURCE_PREFIX)src/rpc/transfer_stream_codec.cpp \
	$(SOURCE_PREFIX)src/rpc/transfer_stream_io.cpp \
	$(SOURCE_PREFIX)src/rpc/transfer_request_utils.cpp

PORT_FORWARD_SRCS = \
	$(SOURCE_PREFIX)src/port_forward/port_forward_endpoint.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_forward_error.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_forward_socket_ops.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_common.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_frame.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_sender.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_service.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_service_expiry.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_session.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_session_teardown.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_spawn.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_streams.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_transport.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_tcp.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_udp.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_workers.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_error.cpp

BASE64_SRCS = $(SOURCE_PREFIX)src/codec/base64_codec.cpp

PATH_UTILS_SRCS = $(SOURCE_PREFIX)src/platform/path_utils.cpp

WIN32_ERROR_SRCS = $(SOURCE_PREFIX)src/platform/win32_error.cpp

SESSION_STORE_SUPPORT_SRCS = \
	$(SOURCE_PREFIX)src/exec/output_renderer.cpp \
	$(SOURCE_PREFIX)src/exec/session_response_builder.cpp

SESSION_STORE_SRCS = \
	$(SESSION_STORE_SUPPORT_SRCS) \
	$(SOURCE_PREFIX)src/exec/session_store.cpp \
	$(SOURCE_PREFIX)src/exec/session_pump.cpp

BASIC_MUTEX_POSIX_SRCS = \
	$(SOURCE_PREFIX)src/platform/basic_mutex.cpp \
	$(SOURCE_PREFIX)src/platform/basic_mutex_posix.cpp

BASIC_MUTEX_WINDOWS_SRCS = \
	$(SOURCE_PREFIX)src/platform/basic_mutex.cpp \
	$(SOURCE_PREFIX)src/platform/basic_mutex_win32.cpp

WAKEUP_PIPE_POSIX_SRCS = $(SOURCE_PREFIX)src/platform/wakeup_pipe.cpp
WAKEUP_PIPE_WINDOWS_SRCS = $(SOURCE_PREFIX)src/platform/wakeup_pipe_win32.cpp

SOCKET_COMMON_SRCS = $(SOURCE_PREFIX)src/platform/socket.cpp
SOCKET_POSIX_SRCS = \
	$(SOCKET_COMMON_SRCS) \
	$(SOURCE_PREFIX)src/platform/socket_posix.cpp
SOCKET_WINDOWS_SRCS = \
	$(SOCKET_COMMON_SRCS) \
	$(SOURCE_PREFIX)src/platform/socket_win32.cpp

SERVER_TRANSPORT_POSIX_SRCS = \
	$(SOCKET_POSIX_SRCS) \
	$(SOURCE_PREFIX)src/http/server_transport.cpp \
	$(SOURCE_PREFIX)src/http/server_transport_posix.cpp

SERVER_TRANSPORT_WINDOWS_SRCS = \
	$(SOCKET_WINDOWS_SRCS) \
	$(SOURCE_PREFIX)src/http/server_transport.cpp \
	$(SOURCE_PREFIX)src/http/server_transport_win32.cpp

BASE_COMMON_SRCS_NO_MAIN = \
	$(CONFIG_SRCS) \
	$(HTTP_SRCS) \
	$(LOGGING_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(PLATFORM_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(SHELL_POLICY_SRCS) \
	$(RUNTIME_SRCS) \
	$(SESSION_STORE_SRCS) \
	$(CAPABILITIES_SRCS) \
	$(PATCH_SRCS) \
	$(ROUTE_SRCS) \
	$(PORT_FORWARD_SRCS) \
	$(BASE64_SRCS) \
	$(TRANSFER_SRCS) \
	$(POLICY_SRCS) \
	$(RPC_FAILURE_SRCS)

POSIX_BASE_SRCS_NO_MAIN = \
	$(BASE_COMMON_SRCS_NO_MAIN) \
	$(SERVER_TRANSPORT_POSIX_SRCS) \
	$(BASIC_MUTEX_POSIX_SRCS) \
	$(WAKEUP_PIPE_POSIX_SRCS)

WINDOWS_BASE_SRCS_NO_MAIN = \
	$(BASE_COMMON_SRCS_NO_MAIN) \
	$(SERVER_TRANSPORT_WINDOWS_SRCS) \
	$(WIN32_ERROR_SRCS) \
	$(BASIC_MUTEX_WINDOWS_SRCS) \
	$(WAKEUP_PIPE_WINDOWS_SRCS)

POSIX_SRCS = \
	$(POSIX_BASE_SRCS_NO_MAIN) \
	$(SOURCE_PREFIX)src/main.cpp \
	$(POSIX_PROCESS_SESSION_SRCS)

HOST_PATCH_SRCS = \
	$(SOURCE_PREFIX)tests/test_patch.cpp \
	$(PATCH_SRCS) \
	$(PLATFORM_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(SOURCE_PREFIX)src/policy/path_policy.cpp

WINDOWS_PATCH_TEST_SRCS = \
	$(HOST_PATCH_SRCS) \
	$(WIN32_ERROR_SRCS)

HOST_TRANSFER_SRCS = \
	$(SOURCE_PREFIX)tests/test_transfer.cpp \
	$(PATH_UTILS_SRCS) \
	$(TRANSFER_SRCS) \
	$(RPC_FAILURE_SRCS)

WINDOWS_TRANSFER_TEST_SRCS = \
	$(HOST_TRANSFER_SRCS) \
	$(WIN32_ERROR_SRCS)

HOST_CONFIG_SRCS = \
	$(SOURCE_PREFIX)tests/test_config.cpp \
	$(CONFIG_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(TEXT_UTILS_SRCS)

WINDOWS_CONFIG_TEST_SRCS = \
	$(HOST_CONFIG_SRCS) \
	$(WIN32_ERROR_SRCS)

HOST_BASIC_MUTEX_TEST_COMMON_SRCS = \
	$(SOURCE_PREFIX)tests/test_basic_mutex.cpp \
	$(PLATFORM_SRCS) \
	$(TEXT_UTILS_SRCS)

HOST_BASIC_MUTEX_SRCS = \
	$(HOST_BASIC_MUTEX_TEST_COMMON_SRCS) \
	$(BASIC_MUTEX_POSIX_SRCS)

WINDOWS_BASIC_MUTEX_TEST_SRCS = \
	$(HOST_BASIC_MUTEX_TEST_COMMON_SRCS) \
	$(BASIC_MUTEX_WINDOWS_SRCS)

WINDOWS_CONSOLE_OUTPUT_TEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_console_output.cpp \
	$(SOURCE_PREFIX)src/exec/console_output.cpp \
	$(PLATFORM_SRCS) \
	$(WIN32_ERROR_SRCS) \
	$(LOGGING_SRCS) \
	$(TEXT_UTILS_SRCS)

WINDOWS_WINSOCK1_SOCKET_BACKEND_TEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_winsock1_socket_backend.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_forward_endpoint.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_forward_error.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_forward_socket_ops.cpp \
	$(SOCKET_WINDOWS_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(WIN32_ERROR_SRCS)

HOST_HTTP_REQUEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_http_request.cpp \
	$(SOURCE_PREFIX)src/http/http_codec.cpp \
	$(SOURCE_PREFIX)src/http/http_request.cpp \
	$(SOURCE_PREFIX)src/http/http_helpers.cpp \
	$(TEXT_UTILS_SRCS)

HOST_SERVER_TRANSPORT_TEST_COMMON_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_transport.cpp \
	$(SOURCE_PREFIX)src/http/http_codec.cpp \
	$(SOURCE_PREFIX)src/http/http_request.cpp \
	$(SOURCE_PREFIX)src/http/http_helpers.cpp \
	$(PLATFORM_SRCS) \
	$(TEXT_UTILS_SRCS)

HOST_SERVER_TRANSPORT_SRCS = \
	$(HOST_SERVER_TRANSPORT_TEST_COMMON_SRCS) \
	$(SERVER_TRANSPORT_POSIX_SRCS)

WINDOWS_SERVER_TRANSPORT_TEST_SRCS = \
	$(HOST_SERVER_TRANSPORT_TEST_COMMON_SRCS) \
	$(SERVER_TRANSPORT_WINDOWS_SRCS) \
	$(LOGGING_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(WIN32_ERROR_SRCS)

SERVER_STREAMING_TEST_COMMON_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_streaming.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_shared.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_routes.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_protocol.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_tcp.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_udp.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_limits.cpp \
	$(SOURCE_PREFIX)tests/test_server_streaming_lifecycle.cpp

HOST_SERVER_STREAMING_SRCS = \
	$(SERVER_STREAMING_TEST_COMMON_SRCS) \
	$(POSIX_BASE_SRCS_NO_MAIN) \
	$(POSIX_PROCESS_SESSION_SRCS)

WINDOWS_SERVER_STREAMING_SRCS = \
	$(SERVER_STREAMING_TEST_COMMON_SRCS) \
	$(WINDOWS_BASE_SRCS_NO_MAIN) \
	$(WINDOWS_DAEMON_SUPPORT_SRCS_NO_ERROR)

HOST_SESSION_STORE_SRCS = \
	$(SOURCE_PREFIX)tests/test_session_store.cpp \
	$(SESSION_STORE_SRCS) \
	$(DAEMON_THREAD_SRCS) \
	$(POSIX_PROCESS_SESSION_SRCS) \
	$(PLATFORM_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(SHELL_POLICY_SRCS) \
	$(BASIC_MUTEX_POSIX_SRCS) \
	$(LOGGING_SRCS) \
	$(CONFIG_SRCS) \
	$(TEXT_UTILS_SRCS)

HOST_CONNECTION_MANAGER_TEST_COMMON_SRCS = \
	$(SOURCE_PREFIX)tests/test_connection_manager.cpp \
	$(SOURCE_PREFIX)src/runtime/connection_manager.cpp \
	$(SOURCE_PREFIX)src/http/http_codec.cpp \
	$(SOURCE_PREFIX)src/http/http_request.cpp \
	$(SOURCE_PREFIX)src/http/http_helpers.cpp \
	$(TEXT_UTILS_SRCS) \
	$(PLATFORM_SRCS) \
	$(LOGGING_SRCS) \
	$(DAEMON_THREAD_SRCS)

HOST_CONNECTION_MANAGER_SRCS = \
	$(HOST_CONNECTION_MANAGER_TEST_COMMON_SRCS) \
	$(SERVER_TRANSPORT_POSIX_SRCS) \
	$(BASIC_MUTEX_POSIX_SRCS)

WINDOWS_CONNECTION_MANAGER_TEST_SRCS = \
	$(HOST_CONNECTION_MANAGER_TEST_COMMON_SRCS) \
	$(SERVER_TRANSPORT_WINDOWS_SRCS) \
	$(BASIC_MUTEX_WINDOWS_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(WIN32_ERROR_SRCS)

SERVER_RUNTIME_TEST_COMMON_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_runtime.cpp

HOST_SERVER_RUNTIME_TEST_SUPPORT_SRCS = \
	$(SERVER_RUNTIME_TEST_COMMON_SRCS) \
	$(POSIX_BASE_SRCS_NO_MAIN)

WINDOWS_SERVER_RUNTIME_TEST_SUPPORT_SRCS = \
	$(SERVER_RUNTIME_TEST_COMMON_SRCS) \
	$(WINDOWS_BASE_SRCS_NO_MAIN)

SERVER_ROUTES_TEST_COMMON_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_routes_shared.cpp \
	$(CAPABILITIES_SRCS) \
	$(ROUTE_SRCS) \
	$(SOURCE_PREFIX)src/http/http_codec.cpp \
	$(SOURCE_PREFIX)src/http/http_helpers.cpp \
	$(SESSION_STORE_SRCS) \
	$(PLATFORM_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(SHELL_POLICY_SRCS) \
	$(PATCH_SRCS) \
	$(LOGGING_SRCS) \
	$(CONFIG_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(DAEMON_THREAD_SRCS) \
	$(TRANSFER_SRCS) \
	$(POLICY_SRCS) \
	$(RPC_FAILURE_SRCS) \
	$(PORT_FORWARD_SRCS) \
	$(BASE64_SRCS)

HOST_SERVER_ROUTES_TEST_SUPPORT_SRCS = \
	$(SERVER_ROUTES_TEST_COMMON_SRCS) \
	$(SERVER_TRANSPORT_POSIX_SRCS) \
	$(BASIC_MUTEX_POSIX_SRCS) \
	$(WAKEUP_PIPE_POSIX_SRCS)

WINDOWS_SERVER_ROUTES_TEST_SUPPORT_SRCS = \
	$(SERVER_ROUTES_TEST_COMMON_SRCS) \
	$(SERVER_TRANSPORT_WINDOWS_SRCS) \
	$(WIN32_ERROR_SRCS) \
	$(BASIC_MUTEX_WINDOWS_SRCS) \
	$(WAKEUP_PIPE_WINDOWS_SRCS)

HOST_SERVER_RUNTIME_SRCS = \
	$(HOST_SERVER_RUNTIME_TEST_SUPPORT_SRCS) \
	$(POSIX_PROCESS_SESSION_SRCS)

HOST_SERVER_ROUTES_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_routes.cpp \
	$(HOST_SERVER_ROUTES_TEST_SUPPORT_SRCS) \
	$(POSIX_PROCESS_SESSION_SRCS)

HOST_SERVER_ROUTES_COMMON_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_routes_common.cpp \
	$(HOST_SERVER_ROUTES_TEST_SUPPORT_SRCS)

HOST_SANDBOX_SRCS = \
	$(SOURCE_PREFIX)tests/test_sandbox.cpp \
	$(LOGGING_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(POLICY_SRCS)

WINDOWS_SANDBOX_TEST_SRCS = \
	$(HOST_SANDBOX_SRCS) \
	$(WIN32_ERROR_SRCS)

HOST_PORT_TUNNEL_FRAME_SRCS = \
	$(SOURCE_PREFIX)tests/test_port_tunnel_frame.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_frame.cpp

WINDOWS_DAEMON_SUPPORT_SRCS_NO_ERROR = \
	$(SOURCE_PREFIX)src/exec/process_session_win32.cpp \
	$(SOURCE_PREFIX)src/exec/console_output.cpp

WINDOWS_DAEMON_SUPPORT_SRCS = \
	$(WINDOWS_DAEMON_SUPPORT_SRCS_NO_ERROR) \
	$(WIN32_ERROR_SRCS)

WINPTY_VENDOR_DIR = $(SOURCE_PREFIX)../../winptyrs/crates/winptyrs-sys/vendor/winpty
WINPTY_VENDOR_SRC_DIR = $(WINPTY_VENDOR_DIR)/src

WINPTY_LIBWINPTY_STEMS = \
	libwinpty/AgentLocation \
	libwinpty/winpty \
	shared/BackgroundDesktop \
	shared/Buffer \
	shared/DebugClient \
	shared/GenRandom \
	shared/OwnedHandle \
	shared/StringUtil \
	shared/WindowsSecurity \
	shared/WindowsVersion \
	shared/WinptyAssert \
	shared/WinptyException \
	shared/WinptyVersion

WINPTY_AGENT_STEMS = \
	agent/Agent \
	agent/AgentCreateDesktop \
	agent/ConsoleFont \
	agent/ConsoleInput \
	agent/ConsoleInputReencoding \
	agent/ConsoleLine \
	agent/DebugShowInput \
	agent/DefaultInputMap \
	agent/EventLoop \
	agent/InputMap \
	agent/LargeConsoleRead \
	agent/NamedPipe \
	agent/Scraper \
	agent/Terminal \
	agent/Win32Console \
	agent/Win32ConsoleBuffer \
	agent/main \
	shared/BackgroundDesktop \
	shared/Buffer \
	shared/DebugClient \
	shared/GenRandom \
	shared/OwnedHandle \
	shared/StringUtil \
	shared/WindowsSecurity \
	shared/WindowsVersion \
	shared/WinptyAssert \
	shared/WinptyException \
	shared/WinptyVersion

WINDOWS_DAEMON_SRCS = \
	$(WINDOWS_BASE_SRCS_NO_MAIN) \
	$(SOURCE_PREFIX)src/main.cpp \
	$(WINDOWS_DAEMON_SUPPORT_SRCS_NO_ERROR)

WINDOWS_SESSION_STORE_TEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_session_store.cpp \
	$(SESSION_STORE_SRCS) \
	$(WINDOWS_DAEMON_SUPPORT_SRCS) \
	$(PLATFORM_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(SHELL_POLICY_SRCS) \
	$(BASIC_MUTEX_WINDOWS_SRCS) \
	$(LOGGING_SRCS) \
	$(CONFIG_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(DAEMON_THREAD_SRCS)

WINDOWS_SERVER_ROUTES_COMMON_TEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_routes_common.cpp \
	$(WINDOWS_SERVER_ROUTES_TEST_SUPPORT_SRCS) \
	$(WINDOWS_DAEMON_SUPPORT_SRCS_NO_ERROR)

WINDOWS_SERVER_ROUTES_TEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_server_routes.cpp \
	$(WINDOWS_SERVER_ROUTES_TEST_SUPPORT_SRCS) \
	$(WINDOWS_DAEMON_SUPPORT_SRCS_NO_ERROR)

WINDOWS_SERVER_RUNTIME_TEST_SRCS = \
	$(WINDOWS_SERVER_RUNTIME_TEST_SUPPORT_SRCS) \
	$(WINDOWS_DAEMON_SUPPORT_SRCS_NO_ERROR)

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
	$(SOURCE_PREFIX)tests/test_session_store.cpp \
	$(WINDOWS_SERVER_STREAMING_SRCS)

WINDOWS_CAPABLE_ROUTE_TEST_SRCS = \
	$(WINDOWS_SERVER_ROUTES_COMMON_TEST_SRCS) \
	$(WINDOWS_SERVER_ROUTES_TEST_SRCS)

WINDOWS_CAPABLE_SERVER_SMOKE_TEST_SRCS = \
	$(WINDOWS_SERVER_RUNTIME_TEST_SRCS)

POSIX_ONLY_TEST_SRCS = \
	$(HOST_SERVER_STREAMING_SRCS) \
	$(HOST_SERVER_ROUTES_SRCS)
