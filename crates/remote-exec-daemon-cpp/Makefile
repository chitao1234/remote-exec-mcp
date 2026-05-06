CXX = i686-w64-mingw32-g++
HOST_CXX ?= g++
MAKEFILE_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
BUILD_DIR ?= $(MAKEFILE_DIR)build
OBJ_DIR := $(BUILD_DIR)/obj
HOST_PROD_OBJ_DIR := $(OBJ_DIR)/host-prod
HOST_TEST_OBJ_DIR := $(OBJ_DIR)/host-test
WINDOWS_XP_PROD_OBJ_DIR := $(OBJ_DIR)/windows-xp-prod
WINDOWS_XP_TEST_OBJ_DIR := $(OBJ_DIR)/windows-xp-test
POSIX_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp
WINDOWS_XP_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp-xp.exe
COMMON_CPPFLAGS := -I$(MAKEFILE_DIR)include -I$(MAKEFILE_DIR)third_party
PROD_CXXFLAGS := -std=c++11 -O0 -Wall -Wextra
TEST_CXXFLAGS := -std=gnu++17 -O0 -Wall -Wextra
HOST_PROD_CPPFLAGS := $(COMMON_CPPFLAGS)
HOST_PROD_CXXFLAGS := $(PROD_CXXFLAGS)
HOST_TEST_CPPFLAGS := $(COMMON_CPPFLAGS)
HOST_TEST_CXXFLAGS := $(TEST_CXXFLAGS)
WINDOWS_XP_PROD_CPPFLAGS := $(COMMON_CPPFLAGS) -DWINVER=0x0501 -D_WIN32_WINNT=0x0501
WINDOWS_XP_PROD_CXXFLAGS := $(PROD_CXXFLAGS)
WINDOWS_XP_TEST_CPPFLAGS := $(COMMON_CPPFLAGS) -DWINVER=0x0501 -D_WIN32_WINNT=0x0501
WINDOWS_XP_TEST_CXXFLAGS := $(TEST_CXXFLAGS)
WINDOWS_XP_LDFLAGS := -static-libgcc -static-libstdc++
DEPFLAGS := -MMD -MP
WIN_LIBS := -lws2_32
TRANSFER_SRCS := $(addprefix $(MAKEFILE_DIR),src/transfer_ops.cpp src/transfer_ops_fs.cpp src/transfer_ops_tar.cpp src/transfer_ops_export.cpp src/transfer_ops_import.cpp src/transfer_glob.cpp)
POLICY_SRCS := $(addprefix $(MAKEFILE_DIR),src/path_policy.cpp src/filesystem_sandbox.cpp)
RPC_FAILURE_SRCS := $(MAKEFILE_DIR)src/rpc_failures.cpp
ROUTE_SRCS := $(addprefix $(MAKEFILE_DIR),src/server_routes.cpp src/server_route_common.cpp src/server_route_exec.cpp src/server_route_image.cpp src/server_route_port_forward.cpp src/server_route_transfer.cpp src/transfer_http_codec.cpp)
PORT_FORWARD_SRCS := $(addprefix $(MAKEFILE_DIR),src/port_forward.cpp src/port_forward_endpoint.cpp src/port_forward_codec.cpp src/port_forward_socket_ops.cpp)
BASE_SRCS := $(addprefix $(MAKEFILE_DIR),src/config.cpp src/http_codec.cpp src/http_connection.cpp src/http_helpers.cpp src/http_request.cpp src/logging.cpp src/text_utils.cpp src/platform.cpp src/shell_policy.cpp src/server.cpp src/server_request_utils.cpp src/server_transport.cpp src/session_store.cpp src/patch_engine.cpp src/basic_mutex.cpp) $(ROUTE_SRCS) $(PORT_FORWARD_SRCS) $(TRANSFER_SRCS) $(POLICY_SRCS) $(RPC_FAILURE_SRCS)
POSIX_SRCS := $(BASE_SRCS) $(addprefix $(MAKEFILE_DIR),src/main.cpp src/process_session_posix.cpp)
WINDOWS_XP_SRCS := $(BASE_SRCS) $(addprefix $(MAKEFILE_DIR),src/main.cpp src/process_session_win32.cpp src/console_output.cpp src/win32_error.cpp)
HOST_PATCH := $(BUILD_DIR)/test_patch
HOST_TRANSFER := $(BUILD_DIR)/test_transfer
HOST_CONFIG := $(BUILD_DIR)/test_config
HOST_HTTP_REQUEST := $(BUILD_DIR)/test_http_request
HOST_SERVER_TRANSPORT := $(BUILD_DIR)/test_server_transport
HOST_SERVER_STREAMING := $(BUILD_DIR)/test_server_streaming
HOST_SESSION_STORE := $(BUILD_DIR)/test_session_store
HOST_SERVER_ROUTES := $(BUILD_DIR)/test_server_routes
HOST_SANDBOX := $(BUILD_DIR)/test_sandbox
XP_SESSION_STORE := $(BUILD_DIR)/test_session_store-xp.exe
XP_TRANSFER := $(BUILD_DIR)/test_transfer-xp.exe
WINE ?= wine

HOST_PATCH_SRCS := $(addprefix $(MAKEFILE_DIR),tests/test_patch.cpp src/patch_engine.cpp src/platform.cpp src/path_policy.cpp)
HOST_TRANSFER_SRCS := $(MAKEFILE_DIR)tests/test_transfer.cpp $(TRANSFER_SRCS) $(RPC_FAILURE_SRCS)
HOST_CONFIG_SRCS := $(addprefix $(MAKEFILE_DIR),tests/test_config.cpp src/config.cpp src/text_utils.cpp)
HOST_HTTP_REQUEST_SRCS := $(addprefix $(MAKEFILE_DIR),tests/test_http_request.cpp src/http_codec.cpp src/http_request.cpp src/http_helpers.cpp src/text_utils.cpp)
HOST_SERVER_TRANSPORT_SRCS := $(addprefix $(MAKEFILE_DIR),tests/test_server_transport.cpp src/http_codec.cpp src/server_transport.cpp src/http_request.cpp src/http_helpers.cpp src/text_utils.cpp)
HOST_SERVER_STREAMING_SRCS := $(MAKEFILE_DIR)tests/test_server_streaming.cpp $(ROUTE_SRCS) $(addprefix $(MAKEFILE_DIR),src/server.cpp src/http_codec.cpp src/http_connection.cpp src/http_helpers.cpp src/http_request.cpp src/session_store.cpp src/process_session_posix.cpp src/platform.cpp src/shell_policy.cpp src/patch_engine.cpp src/server_request_utils.cpp src/server_transport.cpp src/basic_mutex.cpp src/logging.cpp src/config.cpp src/text_utils.cpp) $(TRANSFER_SRCS) $(POLICY_SRCS) $(RPC_FAILURE_SRCS) $(PORT_FORWARD_SRCS)
HOST_SESSION_STORE_SRCS := $(addprefix $(MAKEFILE_DIR),tests/test_session_store.cpp src/session_store.cpp src/process_session_posix.cpp src/platform.cpp src/shell_policy.cpp src/basic_mutex.cpp src/logging.cpp src/config.cpp src/text_utils.cpp)
HOST_SERVER_ROUTES_SRCS := $(MAKEFILE_DIR)tests/test_server_routes.cpp $(ROUTE_SRCS) $(addprefix $(MAKEFILE_DIR),src/http_codec.cpp src/http_helpers.cpp src/session_store.cpp src/process_session_posix.cpp src/platform.cpp src/shell_policy.cpp src/patch_engine.cpp src/server_request_utils.cpp src/server_transport.cpp src/basic_mutex.cpp src/logging.cpp src/config.cpp src/text_utils.cpp) $(TRANSFER_SRCS) $(POLICY_SRCS) $(RPC_FAILURE_SRCS) $(PORT_FORWARD_SRCS)
HOST_SANDBOX_SRCS := $(MAKEFILE_DIR)tests/test_sandbox.cpp $(POLICY_SRCS)
XP_SESSION_STORE_SRCS := $(addprefix $(MAKEFILE_DIR),tests/test_session_store.cpp src/session_store.cpp src/process_session_win32.cpp src/platform.cpp src/shell_policy.cpp src/console_output.cpp src/basic_mutex.cpp src/logging.cpp src/win32_error.cpp src/config.cpp src/text_utils.cpp)
XP_TRANSFER_SRCS := $(MAKEFILE_DIR)tests/test_transfer.cpp $(TRANSFER_SRCS) $(RPC_FAILURE_SRCS)

host_prod_objs = $(patsubst $(MAKEFILE_DIR)%.cpp,$(HOST_PROD_OBJ_DIR)/%.o,$(1))
host_test_objs = $(patsubst $(MAKEFILE_DIR)%.cpp,$(HOST_TEST_OBJ_DIR)/%.o,$(1))
windows_xp_prod_objs = $(patsubst $(MAKEFILE_DIR)%.cpp,$(WINDOWS_XP_PROD_OBJ_DIR)/%.o,$(1))
windows_xp_test_objs = $(patsubst $(MAKEFILE_DIR)%.cpp,$(WINDOWS_XP_TEST_OBJ_DIR)/%.o,$(1))

POSIX_OBJS := $(sort $(call host_prod_objs,$(POSIX_SRCS)))
WINDOWS_XP_OBJS := $(sort $(call windows_xp_prod_objs,$(WINDOWS_XP_SRCS)))
HOST_PATCH_OBJS := $(sort $(call host_test_objs,$(HOST_PATCH_SRCS)))
HOST_TRANSFER_OBJS := $(sort $(call host_test_objs,$(HOST_TRANSFER_SRCS)))
HOST_CONFIG_OBJS := $(sort $(call host_test_objs,$(HOST_CONFIG_SRCS)))
HOST_HTTP_REQUEST_OBJS := $(sort $(call host_test_objs,$(HOST_HTTP_REQUEST_SRCS)))
HOST_SERVER_TRANSPORT_OBJS := $(sort $(call host_test_objs,$(HOST_SERVER_TRANSPORT_SRCS)))
HOST_SERVER_STREAMING_OBJS := $(sort $(call host_test_objs,$(HOST_SERVER_STREAMING_SRCS)))
HOST_SESSION_STORE_OBJS := $(sort $(call host_test_objs,$(HOST_SESSION_STORE_SRCS)))
HOST_SERVER_ROUTES_OBJS := $(sort $(call host_test_objs,$(HOST_SERVER_ROUTES_SRCS)))
HOST_SANDBOX_OBJS := $(sort $(call host_test_objs,$(HOST_SANDBOX_SRCS)))
XP_SESSION_STORE_OBJS := $(sort $(call windows_xp_test_objs,$(XP_SESSION_STORE_SRCS)))
XP_TRANSFER_OBJS := $(sort $(call windows_xp_test_objs,$(XP_TRANSFER_SRCS)))

DEP_FILES := $(sort \
	$(POSIX_OBJS:.o=.d) \
	$(WINDOWS_XP_OBJS:.o=.d) \
	$(HOST_PATCH_OBJS:.o=.d) \
	$(HOST_TRANSFER_OBJS:.o=.d) \
	$(HOST_CONFIG_OBJS:.o=.d) \
	$(HOST_HTTP_REQUEST_OBJS:.o=.d) \
	$(HOST_SERVER_TRANSPORT_OBJS:.o=.d) \
	$(HOST_SERVER_STREAMING_OBJS:.o=.d) \
	$(HOST_SESSION_STORE_OBJS:.o=.d) \
	$(HOST_SERVER_ROUTES_OBJS:.o=.d) \
	$(HOST_SANDBOX_OBJS:.o=.d) \
	$(XP_SESSION_STORE_OBJS:.o=.d) \
	$(XP_TRANSFER_OBJS:.o=.d))

all: all-posix

all-posix: $(POSIX_TARGET)

all-windows-xp: $(WINDOWS_XP_TARGET)

$(POSIX_TARGET): $(POSIX_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_PROD_CXXFLAGS) -o $@ $^

$(WINDOWS_XP_TARGET): $(WINDOWS_XP_OBJS)
	mkdir -p $(dir $@)
	$(CXX) $(WINDOWS_XP_PROD_CXXFLAGS) $(WINDOWS_XP_LDFLAGS) -o $@ $^ $(WIN_LIBS)

$(HOST_PROD_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_PROD_CPPFLAGS) $(HOST_PROD_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(HOST_TEST_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CPPFLAGS) $(HOST_TEST_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(WINDOWS_XP_PROD_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(CXX) $(WINDOWS_XP_PROD_CPPFLAGS) $(WINDOWS_XP_PROD_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(WINDOWS_XP_TEST_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(CXX) $(WINDOWS_XP_TEST_CPPFLAGS) $(WINDOWS_XP_TEST_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

clean:
	rm -rf $(BUILD_DIR)

test-host-patch: $(HOST_PATCH)
	$(HOST_PATCH)

$(HOST_PATCH): $(HOST_PATCH_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^

test-host-transfer: $(HOST_TRANSFER)
	$(HOST_TRANSFER)

$(HOST_TRANSFER): $(HOST_TRANSFER_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^

test-host-config: $(HOST_CONFIG)
	$(HOST_CONFIG)

$(HOST_CONFIG): $(HOST_CONFIG_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^

test-host-http-request: $(HOST_HTTP_REQUEST)
	$(HOST_HTTP_REQUEST)

$(HOST_HTTP_REQUEST): $(HOST_HTTP_REQUEST_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^

test-host-server-transport: $(HOST_SERVER_TRANSPORT)
	$(HOST_SERVER_TRANSPORT)

$(HOST_SERVER_TRANSPORT): $(HOST_SERVER_TRANSPORT_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^

test-host-server-streaming: $(HOST_SERVER_STREAMING)
	$(HOST_SERVER_STREAMING)

$(HOST_SERVER_STREAMING): $(HOST_SERVER_STREAMING_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^

test-host-session-store: $(HOST_SESSION_STORE)
	$(HOST_SESSION_STORE)

$(HOST_SESSION_STORE): $(HOST_SESSION_STORE_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^

test-host-server-routes: $(HOST_SERVER_ROUTES)
	$(HOST_SERVER_ROUTES)

$(HOST_SERVER_ROUTES): $(HOST_SERVER_ROUTES_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^

test-host-sandbox: $(HOST_SANDBOX)
	$(HOST_SANDBOX)

$(HOST_SANDBOX): $(HOST_SANDBOX_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CXXFLAGS) -o $@ $^

test-wine-session-store: $(XP_SESSION_STORE)
	$(WINE) $(XP_SESSION_STORE)

$(XP_SESSION_STORE): $(XP_SESSION_STORE_OBJS)
	mkdir -p $(dir $@)
	$(CXX) $(WINDOWS_XP_TEST_CXXFLAGS) $(WINDOWS_XP_LDFLAGS) -o $@ $^ $(WIN_LIBS)

test-wine-transfer: $(XP_TRANSFER)
	$(WINE) $(XP_TRANSFER)

$(XP_TRANSFER): $(XP_TRANSFER_OBJS)
	mkdir -p $(dir $@)
	$(CXX) $(WINDOWS_XP_TEST_CXXFLAGS) $(WINDOWS_XP_LDFLAGS) -o $@ $^ $(WIN_LIBS)

check-posix: test-host-patch test-host-transfer test-host-config test-host-http-request test-host-server-transport test-host-server-streaming test-host-session-store test-host-server-routes test-host-sandbox all-posix

check-windows-xp: all-windows-xp $(XP_TRANSFER)

check: check-posix

-include $(DEP_FILES)

.PHONY: all all-posix all-windows-xp clean test-host-patch test-host-transfer test-host-config test-host-http-request test-host-server-transport test-host-server-streaming test-host-session-store test-host-server-routes test-host-sandbox test-wine-session-store test-wine-transfer check-posix check-windows-xp check
