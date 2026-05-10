BUILD_DIR ?= $(MAKEFILE_DIR)build
OBJ_DIR := $(BUILD_DIR)/obj

COMMON_CPPFLAGS := -I$(MAKEFILE_DIR)include -I$(MAKEFILE_DIR)third_party
PROD_CXXFLAGS := -std=c++11 -O0 -Wall -Wextra
TEST_CXXFLAGS := -std=gnu++17 -O0 -Wall -Wextra
DEPFLAGS := -MMD -MP
DEP_FILES :=

TRANSFER_SRCS := \
	$(MAKEFILE_DIR)src/transfer_ops.cpp \
	$(MAKEFILE_DIR)src/transfer_ops_fs.cpp \
	$(MAKEFILE_DIR)src/transfer_ops_tar.cpp \
	$(MAKEFILE_DIR)src/transfer_ops_export.cpp \
	$(MAKEFILE_DIR)src/transfer_ops_import.cpp \
	$(MAKEFILE_DIR)src/transfer_glob.cpp

POLICY_SRCS := \
	$(MAKEFILE_DIR)src/path_policy.cpp \
	$(MAKEFILE_DIR)src/filesystem_sandbox.cpp

RPC_FAILURE_SRCS := $(MAKEFILE_DIR)src/rpc_failures.cpp

ROUTE_SRCS := \
	$(MAKEFILE_DIR)src/server_routes.cpp \
	$(MAKEFILE_DIR)src/server_route_common.cpp \
	$(MAKEFILE_DIR)src/server_route_exec.cpp \
	$(MAKEFILE_DIR)src/server_route_image.cpp \
	$(MAKEFILE_DIR)src/server_route_transfer.cpp \
	$(MAKEFILE_DIR)src/transfer_http_codec.cpp

PORT_FORWARD_SRCS := \
	$(MAKEFILE_DIR)src/port_forward_endpoint.cpp \
	$(MAKEFILE_DIR)src/port_forward_error.cpp \
	$(MAKEFILE_DIR)src/port_forward_socket_ops.cpp \
	$(MAKEFILE_DIR)src/port_tunnel_frame.cpp \
	$(MAKEFILE_DIR)src/port_tunnel.cpp \
	$(MAKEFILE_DIR)src/port_tunnel_session.cpp \
	$(MAKEFILE_DIR)src/port_tunnel_transport.cpp \
	$(MAKEFILE_DIR)src/port_tunnel_tcp.cpp \
	$(MAKEFILE_DIR)src/port_tunnel_udp.cpp \
	$(MAKEFILE_DIR)src/port_tunnel_error.cpp

BASE64_SRCS := $(MAKEFILE_DIR)src/base64_codec.cpp

BASE_SRCS := \
	$(MAKEFILE_DIR)src/config.cpp \
	$(MAKEFILE_DIR)src/http_codec.cpp \
	$(MAKEFILE_DIR)src/http_connection.cpp \
	$(MAKEFILE_DIR)src/http_helpers.cpp \
	$(MAKEFILE_DIR)src/http_request.cpp \
	$(MAKEFILE_DIR)src/logging.cpp \
	$(MAKEFILE_DIR)src/text_utils.cpp \
	$(MAKEFILE_DIR)src/platform.cpp \
	$(MAKEFILE_DIR)src/shell_policy.cpp \
	$(MAKEFILE_DIR)src/server.cpp \
	$(MAKEFILE_DIR)src/server_request_utils.cpp \
	$(MAKEFILE_DIR)src/server_runtime.cpp \
	$(MAKEFILE_DIR)src/server_transport.cpp \
	$(MAKEFILE_DIR)src/session_store.cpp \
	$(MAKEFILE_DIR)src/patch_engine.cpp \
	$(MAKEFILE_DIR)src/basic_mutex.cpp \
	$(MAKEFILE_DIR)src/connection_manager.cpp \
	$(ROUTE_SRCS) \
	$(PORT_FORWARD_SRCS) \
	$(BASE64_SRCS) \
	$(TRANSFER_SRCS) \
	$(POLICY_SRCS) \
	$(RPC_FAILURE_SRCS)

cpp_objs = $(patsubst $(MAKEFILE_DIR)%.cpp,$(1)/%.o,$(2))

define run_test
$1: $2
	$2
endef
