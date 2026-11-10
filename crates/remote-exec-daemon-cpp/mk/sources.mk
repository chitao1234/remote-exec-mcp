# Shared C++ source inventory.
#
# This file intentionally uses make syntax accepted by GNU make, BSD make, and
# NMAKE. Rule logic stays in dialect-specific makefiles.
#
# TEXT_ENCODING_ACTIVE_SRCS is intentionally left empty here. Each dialect
# makefile sets it after including this file, using its own conditional syntax,
# because the feature can be compiled out.

UCHARDET_SRC_DIR = $(SOURCE_PREFIX)third_party/uchardet/src

TRANSFER_SRCS = \
	$(SOURCE_PREFIX)src/transfer/transfer_archive_io.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_ops.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_archive.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_ops_fs.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_ops_tar.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_ops_export.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_ops_import.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_import_materializer.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_import_plan.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_options.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_warning_codec.cpp \
	$(SOURCE_PREFIX)src/transfer/transfer_glob.cpp

POLICY_SRCS = \
	$(SOURCE_PREFIX)src/policy/path_policy.cpp \
	$(SOURCE_PREFIX)src/policy/path_compare.cpp \
	$(SOURCE_PREFIX)src/policy/file_ops.cpp \
	$(SOURCE_PREFIX)src/policy/filesystem_sandbox.cpp

RPC_FAILURE_SRCS = $(SOURCE_PREFIX)src/rpc/rpc_failures.cpp

TRANSFER_WIRE_SRCS = $(SOURCE_PREFIX)src/rpc/transfer_wire.cpp

PATCH_SRCS = $(SOURCE_PREFIX)src/patch/patch_engine.cpp

APPLY_PATCH_CLI_SRCS = \
	$(SOURCE_PREFIX)src/apply_patch_main.cpp \
	$(PATCH_SRCS) \
	$(PLATFORM_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(TEXT_ENCODING_ACTIVE_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(SOURCE_PREFIX)src/policy/path_policy.cpp

WINDOWS_APPLY_PATCH_CLI_SRCS = \
	$(APPLY_PATCH_CLI_SRCS) \
	$(WIN32_ERROR_SRCS)

IMAGE_SRCS = $(SOURCE_PREFIX)src/image/image_ops.cpp

CONFIG_SRCS = $(SOURCE_PREFIX)src/core/config.cpp

LOGGING_SRCS = $(SOURCE_PREFIX)src/core/logging.cpp

TEXT_UTILS_SRCS = \
	$(SOURCE_PREFIX)src/core/text_utils.cpp \
	$(SOURCE_PREFIX)src/core/text_file.cpp \
	$(SOURCE_PREFIX)src/core/legacy_encoding_disabled.cpp

# Legacy encoding autodetection and conversion. These are linked only when
# TEXT_ENCODING is on, so a UTF-8-only build carries none of the detector or
# conversion data. The always-present fallbacks above keep text_file.cpp
# link-complete either way.
TEXT_ENCODING_LIB_SRCS = \
	$(SOURCE_PREFIX)src/core/legacy_encoding.cpp \
	$(SOURCE_PREFIX)src/core/encoding_detect.cpp

UCHARDET_SRCS = \
	$(UCHARDET_SRC_DIR)/CharDistribution.cpp \
	$(UCHARDET_SRC_DIR)/JpCntx.cpp \
	$(UCHARDET_SRC_DIR)/nsBig5Prober.cpp \
	$(UCHARDET_SRC_DIR)/nsCharSetProber.cpp \
	$(UCHARDET_SRC_DIR)/nsEscCharsetProber.cpp \
	$(UCHARDET_SRC_DIR)/nsEscSM.cpp \
	$(UCHARDET_SRC_DIR)/nsEUCJPProber.cpp \
	$(UCHARDET_SRC_DIR)/nsEUCKRProber.cpp \
	$(UCHARDET_SRC_DIR)/nsEUCTWProber.cpp \
	$(UCHARDET_SRC_DIR)/nsGB2312Prober.cpp \
	$(UCHARDET_SRC_DIR)/nsHebrewProber.cpp \
	$(UCHARDET_SRC_DIR)/nsLatin1Prober.cpp \
	$(UCHARDET_SRC_DIR)/nsMBCSGroupProber.cpp \
	$(UCHARDET_SRC_DIR)/nsMBCSSM.cpp \
	$(UCHARDET_SRC_DIR)/nsSBCharSetProber.cpp \
	$(UCHARDET_SRC_DIR)/nsSBCSGroupProber.cpp \
	$(UCHARDET_SRC_DIR)/nsSJISProber.cpp \
	$(UCHARDET_SRC_DIR)/nsUniversalDetector.cpp \
	$(UCHARDET_SRC_DIR)/nsUTF8Prober.cpp \
	$(UCHARDET_SRC_DIR)/uchardet.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangArabicModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangBulgarianModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangDanishModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangEsperantoModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangFrenchModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangGermanModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangGreekModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangHebrewModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangHungarianModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangRussianModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangSpanishModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangThaiModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangTurkishModel.cpp \
	$(UCHARDET_SRC_DIR)/LangModels/LangVietnameseModel.cpp

TEXT_ENCODING_SRCS = $(TEXT_ENCODING_LIB_SRCS) $(UCHARDET_SRCS)

SHELL_POLICY_SRCS = $(SOURCE_PREFIX)src/core/shell_policy.cpp

CAPABILITIES_SRCS = $(SOURCE_PREFIX)src/capabilities/daemon_capabilities.cpp

POSIX_CHILD_REAPER_SRCS = $(SOURCE_PREFIX)src/exec/posix_child_reaper.cpp

PROCESS_ENVIRONMENT_SRCS = $(SOURCE_PREFIX)src/exec/process_environment.cpp

POSIX_PROCESS_SESSION_SRCS = \
	$(SOURCE_PREFIX)src/exec/process_session_posix.cpp \
	$(SOURCE_PREFIX)src/exec/locale.cpp \
	$(PROCESS_ENVIRONMENT_SRCS) \
	$(POSIX_CHILD_REAPER_SRCS)

RUNTIME_SRCS = \
	$(SOURCE_PREFIX)src/runtime/server.cpp \
	$(SOURCE_PREFIX)src/runtime/daemon_thread.cpp

PLATFORM_SRCS = $(SOURCE_PREFIX)src/platform/platform.cpp

HTTP_SRCS = \
	$(SOURCE_PREFIX)src/http/http_codec.cpp \
	$(SOURCE_PREFIX)src/http/http_helpers.cpp \
	$(SOURCE_PREFIX)src/http/http_request.cpp

TLS_SRCS = \
	$(SOURCE_PREFIX)src/tls/openssl_compat.cpp \
	$(SOURCE_PREFIX)src/tls/tls_connection_transport.cpp

ROUTE_SRCS = \
	$(SOURCE_PREFIX)src/rpc/capabilities_http_codec.cpp \
	$(SOURCE_PREFIX)src/rpc/exec_http_codec.cpp \
	$(SOURCE_PREFIX)src/rpc/exec_request_utils.cpp \
	$(IMAGE_SRCS) \
	$(SOURCE_PREFIX)src/rpc/server_contract.cpp \
	$(SOURCE_PREFIX)src/rpc/server_request_utils.cpp \
	$(SOURCE_PREFIX)src/rpc/server_route_common.cpp \
	$(SOURCE_PREFIX)src/rpc/server_route_file.cpp \
	$(SOURCE_PREFIX)src/rpc/server_route_image.cpp \
	$(SOURCE_PREFIX)src/rpc/server_route_transfer.cpp \
	$(SOURCE_PREFIX)src/rpc/file_request_utils.cpp \
	$(SOURCE_PREFIX)src/rpc/transfer_http_codec.cpp \
	$(SOURCE_PREFIX)src/rpc/transfer_stream_codec.cpp \
	$(SOURCE_PREFIX)src/rpc/transfer_stream_io.cpp \
	$(TRANSFER_WIRE_SRCS) \
	$(SOURCE_PREFIX)src/rpc/transfer_request_utils.cpp

PORT_FORWARD_SRCS = \
	$(SOURCE_PREFIX)src/port_forward/port_forward_endpoint.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_forward_error.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_forward_socket_ops.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_frame.cpp

BASE64_SRCS = $(SOURCE_PREFIX)src/codec/base64_codec.cpp

PATH_UTILS_SRCS = $(SOURCE_PREFIX)src/platform/path_utils.cpp

WIN32_ERROR_SRCS = $(SOURCE_PREFIX)src/platform/win32_error.cpp

OUTPUT_RENDERER_SRCS = $(SOURCE_PREFIX)src/exec/output_renderer.cpp

BASIC_MUTEX_POSIX_SRCS = \
	$(SOURCE_PREFIX)src/platform/basic_mutex.cpp \
	$(SOURCE_PREFIX)src/platform/basic_mutex_posix.cpp

BASIC_MUTEX_WINDOWS_SRCS = \
	$(SOURCE_PREFIX)src/platform/basic_mutex.cpp \
	$(SOURCE_PREFIX)src/platform/basic_mutex_win32.cpp

SOCKET_COMMON_SRCS = $(SOURCE_PREFIX)src/platform/socket.cpp
SOCKET_POSIX_SRCS = \
	$(SOCKET_COMMON_SRCS) \
	$(SOURCE_PREFIX)src/platform/socket_posix.cpp
SOCKET_WINDOWS_SRCS = \
	$(SOCKET_COMMON_SRCS) \
	$(SOURCE_PREFIX)src/platform/socket_win32.cpp

REACTOR_SRCS = \
	$(SOURCE_PREFIX)src/core/byte_buffer.cpp \
	$(SOURCE_PREFIX)src/core/event_loop_posix.cpp \
	$(SOURCE_PREFIX)src/core/event_loop_win32.cpp \
	$(SOURCE_PREFIX)src/core/socket_stream.cpp \
	$(SOURCE_PREFIX)src/core/blocking_pool.cpp \
	$(SOURCE_PREFIX)src/core/json_value.cpp \
	$(SOURCE_PREFIX)src/exec/bounded_transcript.cpp \
	$(SOURCE_PREFIX)src/exec/exec_service.cpp \
	$(SOURCE_PREFIX)src/http/reactor_http_conn.cpp \
	$(SOURCE_PREFIX)src/port_forward/tunnel_service.cpp \
	$(SOURCE_PREFIX)src/rpc/reactor_router.cpp \
	$(SOURCE_PREFIX)src/runtime/reactor_server.cpp

JSON_SRCS = $(SOURCE_PREFIX)src/core/json_value.cpp

BASE_COMMON_SRCS_NO_MAIN = \
	$(CONFIG_SRCS) \
	$(TLS_SRCS) \
	$(HTTP_SRCS) \
	$(LOGGING_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(TEXT_ENCODING_ACTIVE_SRCS) \
	$(PLATFORM_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(SHELL_POLICY_SRCS) \
	$(RUNTIME_SRCS) \
	$(CAPABILITIES_SRCS) \
	$(PATCH_SRCS) \
	$(ROUTE_SRCS) \
	$(PORT_FORWARD_SRCS) \
	$(BASE64_SRCS) \
	$(TRANSFER_SRCS) \
	$(POLICY_SRCS) \
	$(RPC_FAILURE_SRCS) \
	$(OUTPUT_RENDERER_SRCS) \
	$(REACTOR_SRCS)

POSIX_BASE_SRCS_NO_MAIN = \
	$(BASE_COMMON_SRCS_NO_MAIN) \
	$(SOCKET_POSIX_SRCS) \
	$(BASIC_MUTEX_POSIX_SRCS)

WINDOWS_BASE_SRCS_NO_MAIN = \
	$(BASE_COMMON_SRCS_NO_MAIN) \
	$(SOCKET_WINDOWS_SRCS) \
	$(WIN32_ERROR_SRCS) \
	$(BASIC_MUTEX_WINDOWS_SRCS)

POSIX_SRCS = \
	$(POSIX_BASE_SRCS_NO_MAIN) \
	$(SOURCE_PREFIX)src/main.cpp \
	$(POSIX_PROCESS_SESSION_SRCS)

HOST_PATCH_SRCS = \
	$(SOURCE_PREFIX)tests/test_patch.cpp \
	$(PATCH_SRCS) \
	$(PLATFORM_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(TEXT_ENCODING_ACTIVE_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(SOURCE_PREFIX)src/policy/path_policy.cpp

WINDOWS_PATCH_TEST_SRCS = \
	$(HOST_PATCH_SRCS) \
	$(WIN32_ERROR_SRCS)

HOST_TRANSFER_SRCS = \
	$(SOURCE_PREFIX)tests/test_transfer.cpp \
	$(PATH_UTILS_SRCS) \
	$(TRANSFER_SRCS) \
	$(TRANSFER_WIRE_SRCS) \
	$(RPC_FAILURE_SRCS)

WINDOWS_TRANSFER_TEST_SRCS = \
	$(HOST_TRANSFER_SRCS) \
	$(WIN32_ERROR_SRCS)

HOST_CONFIG_SRCS = \
	$(SOURCE_PREFIX)tests/test_config.cpp \
	$(CONFIG_SRCS) \
	$(PATH_UTILS_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(TEXT_ENCODING_ACTIVE_SRCS) \
	$(SOURCE_PREFIX)src/policy/path_policy.cpp

WINDOWS_CONFIG_TEST_SRCS = \
	$(HOST_CONFIG_SRCS) \
	$(WIN32_ERROR_SRCS)

HOST_BASIC_MUTEX_TEST_COMMON_SRCS = \
	$(SOURCE_PREFIX)tests/test_basic_mutex.cpp \
	$(PLATFORM_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(TEXT_ENCODING_ACTIVE_SRCS)

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
	$(TEXT_UTILS_SRCS) \
	$(TEXT_ENCODING_ACTIVE_SRCS)

WINDOWS_WINSOCK1_SOCKET_BACKEND_TEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_winsock1_socket_backend.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_forward_endpoint.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_forward_error.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_forward_socket_ops.cpp \
	$(SOCKET_WINDOWS_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(TEXT_ENCODING_ACTIVE_SRCS) \
	$(WIN32_ERROR_SRCS)

HOST_HTTP_REQUEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_http_request.cpp \
	$(SOURCE_PREFIX)src/http/http_codec.cpp \
	$(SOURCE_PREFIX)src/http/http_request.cpp \
	$(SOURCE_PREFIX)src/http/http_helpers.cpp \
	$(JSON_SRCS) \
	$(TEXT_UTILS_SRCS) \
	$(TEXT_ENCODING_ACTIVE_SRCS)

HOST_SANDBOX_SRCS = \
	$(SOURCE_PREFIX)tests/test_sandbox.cpp \
	$(POSIX_BASE_SRCS_NO_MAIN) \
	$(POSIX_PROCESS_SESSION_SRCS)

HOST_REACTOR_CORE_SRCS = \
	$(SOURCE_PREFIX)tests/test_reactor_core.cpp \
	$(POSIX_BASE_SRCS_NO_MAIN) \
	$(POSIX_PROCESS_SESSION_SRCS)

WINDOWS_SANDBOX_TEST_SRCS = \
	$(SOURCE_PREFIX)tests/test_sandbox.cpp \
	$(WINDOWS_BASE_SRCS_NO_MAIN) \
	$(WINDOWS_DAEMON_SUPPORT_SRCS_NO_ERROR)

HOST_PORT_TUNNEL_FRAME_SRCS = \
	$(SOURCE_PREFIX)tests/test_port_tunnel_frame.cpp \
	$(SOURCE_PREFIX)src/port_forward/port_tunnel_frame.cpp

WINDOWS_DAEMON_SUPPORT_SRCS_NO_ERROR = \
	$(SOURCE_PREFIX)src/exec/process_session_win32.cpp \
	$(PROCESS_ENVIRONMENT_SRCS) \
	$(SOURCE_PREFIX)src/exec/console_output.cpp \
	$(SOURCE_PREFIX)src/platform/win32_process_tree.cpp

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
	shared/ConsoleWindow \
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
