WINDOWS_XP_CXX ?= i686-w64-mingw32-g++
WINE ?= wine
ifeq ($(OS),Windows_NT)
WINDOWS_XP_TEST_RUNNER :=
else
WINDOWS_XP_TEST_RUNNER ?= $(WINE)
endif

WINDOWS_XP_PROD_OBJ_DIR := $(OBJ_DIR)/windows-xp-prod
WINDOWS_XP_TEST_OBJ_DIR := $(OBJ_DIR)/windows-xp-test
WINDOWS_XP_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp-xp.exe

WINDOWS_XP_PROD_CPPFLAGS := $(COMMON_CPPFLAGS) -DWIN32_LEAN_AND_MEAN -DUNICODE -D_UNICODE -DWINVER=0x0501 -D_WIN32_WINNT=0x0501
WINDOWS_XP_PROD_CXXFLAGS := $(PROD_CXXFLAGS)
WINDOWS_XP_TEST_CPPFLAGS := $(COMMON_CPPFLAGS) -DWIN32_LEAN_AND_MEAN -DUNICODE -D_UNICODE -DWINVER=0x0501 -D_WIN32_WINNT=0x0501
WINDOWS_XP_TEST_CXXFLAGS := $(XP_TEST_CXXFLAGS)
WINDOWS_XP_LDFLAGS := -static-libgcc -static-libstdc++
WINDOWS_XP_LDLIBS := -lws2_32

WINDOWS_XP_SRCS := $(WINDOWS_DAEMON_SRCS)

XP_BASIC_MUTEX := $(BUILD_DIR)/test_basic_mutex-xp.exe
XP_PATCH := $(BUILD_DIR)/test_patch-xp.exe
XP_SESSION_STORE := $(BUILD_DIR)/test_session_store-xp.exe
XP_SERVER_STREAMING := $(BUILD_DIR)/test_server_streaming-xp.exe
XP_TRANSFER := $(BUILD_DIR)/test_transfer-xp.exe
XP_CONFIG := $(BUILD_DIR)/test_config-xp.exe
XP_HTTP_REQUEST := $(BUILD_DIR)/test_http_request-xp.exe
XP_SERVER_TRANSPORT := $(BUILD_DIR)/test_server_transport-xp.exe
XP_CONNECTION_MANAGER := $(BUILD_DIR)/test_connection_manager-xp.exe
XP_SERVER_ROUTES_COMMON := $(BUILD_DIR)/test_server_routes_common-xp.exe
XP_SERVER_ROUTES := $(BUILD_DIR)/test_server_routes-xp.exe
XP_SERVER_RUNTIME := $(BUILD_DIR)/test_server_runtime-xp.exe
XP_SANDBOX := $(BUILD_DIR)/test_sandbox-xp.exe
XP_PORT_TUNNEL_FRAME := $(BUILD_DIR)/test_port_tunnel_frame-xp.exe

WINDOWS_XP_PLATFORM_NEUTRAL_TEST_TARGETS := \
	$(XP_BASIC_MUTEX) \
	$(XP_PATCH) \
	$(XP_TRANSFER) \
	$(XP_CONFIG) \
	$(XP_HTTP_REQUEST) \
	$(XP_SERVER_TRANSPORT) \
	$(XP_CONNECTION_MANAGER) \
	$(XP_SERVER_ROUTES_COMMON) \
	$(XP_SERVER_ROUTES) \
	$(XP_SANDBOX) \
	$(XP_PORT_TUNNEL_FRAME)

WINDOWS_XP_PROCESS_TEST_TARGETS := \
	$(XP_SESSION_STORE) \
	$(XP_SERVER_STREAMING)

WINDOWS_XP_SERVER_SMOKE_TEST_TARGETS := $(XP_SERVER_RUNTIME)

WINDOWS_XP_TEST_TARGETS := \
	$(WINDOWS_XP_PLATFORM_NEUTRAL_TEST_TARGETS) \
	$(WINDOWS_XP_PROCESS_TEST_TARGETS) \
	$(WINDOWS_XP_SERVER_SMOKE_TEST_TARGETS)

XP_BASIC_MUTEX_SRCS := $(WINDOWS_BASIC_MUTEX_TEST_SRCS)

XP_PATCH_SRCS := $(HOST_PATCH_SRCS)

XP_SESSION_STORE_SRCS := $(WINDOWS_SESSION_STORE_TEST_SRCS)

XP_SERVER_STREAMING_SRCS := $(WINDOWS_SERVER_STREAMING_SRCS)

XP_TRANSFER_SRCS := \
	$(MAKEFILE_DIR)tests/test_transfer.cpp \
	$(PATH_UTILS_SRCS) \
	$(TRANSFER_SRCS) \
	$(RPC_FAILURE_SRCS)

XP_CONFIG_SRCS := $(HOST_CONFIG_SRCS)

XP_HTTP_REQUEST_SRCS := $(HOST_HTTP_REQUEST_SRCS)

XP_SERVER_TRANSPORT_SRCS := $(WINDOWS_SERVER_TRANSPORT_TEST_SRCS)

XP_CONNECTION_MANAGER_SRCS := $(WINDOWS_CONNECTION_MANAGER_TEST_SRCS)

XP_SERVER_ROUTES_COMMON_SRCS := $(WINDOWS_SERVER_ROUTES_COMMON_TEST_SRCS)

XP_SERVER_ROUTES_SRCS := $(WINDOWS_SERVER_ROUTES_TEST_SRCS)

XP_SERVER_RUNTIME_SRCS := $(WINDOWS_SERVER_RUNTIME_TEST_SRCS)

XP_SANDBOX_SRCS := $(HOST_SANDBOX_SRCS)

XP_PORT_TUNNEL_FRAME_SRCS := $(HOST_PORT_TUNNEL_FRAME_SRCS)

WINDOWS_XP_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_PROD_OBJ_DIR),$(WINDOWS_XP_SRCS)))
XP_BASIC_MUTEX_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_BASIC_MUTEX_SRCS)))
XP_PATCH_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_PATCH_SRCS)))
XP_SESSION_STORE_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_SESSION_STORE_SRCS)))
XP_SERVER_STREAMING_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_SERVER_STREAMING_SRCS)))
XP_TRANSFER_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_TRANSFER_SRCS)))
XP_CONFIG_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_CONFIG_SRCS)))
XP_HTTP_REQUEST_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_HTTP_REQUEST_SRCS)))
XP_SERVER_TRANSPORT_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_SERVER_TRANSPORT_SRCS)))
XP_CONNECTION_MANAGER_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_CONNECTION_MANAGER_SRCS)))
XP_SERVER_ROUTES_COMMON_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_SERVER_ROUTES_COMMON_SRCS)))
XP_SERVER_ROUTES_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_SERVER_ROUTES_SRCS)))
XP_SERVER_RUNTIME_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_SERVER_RUNTIME_SRCS)))
XP_SANDBOX_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_SANDBOX_SRCS)))
XP_PORT_TUNNEL_FRAME_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_PORT_TUNNEL_FRAME_SRCS)))

DEP_FILES += \
	$(WINDOWS_XP_OBJS:.o=.d) \
	$(XP_BASIC_MUTEX_OBJS:.o=.d) \
	$(XP_PATCH_OBJS:.o=.d) \
	$(XP_SESSION_STORE_OBJS:.o=.d) \
	$(XP_SERVER_STREAMING_OBJS:.o=.d) \
	$(XP_TRANSFER_OBJS:.o=.d) \
	$(XP_CONFIG_OBJS:.o=.d) \
	$(XP_HTTP_REQUEST_OBJS:.o=.d) \
	$(XP_SERVER_TRANSPORT_OBJS:.o=.d) \
	$(XP_CONNECTION_MANAGER_OBJS:.o=.d) \
	$(XP_SERVER_ROUTES_COMMON_OBJS:.o=.d) \
	$(XP_SERVER_ROUTES_OBJS:.o=.d) \
	$(XP_SERVER_RUNTIME_OBJS:.o=.d) \
	$(XP_SANDBOX_OBJS:.o=.d) \
	$(XP_PORT_TUNNEL_FRAME_OBJS:.o=.d)

define run_windows_xp_test
$1: $2
	REMOTE_EXEC_LOG=$$(TEST_LOG_LEVEL) $$(WINDOWS_XP_TEST_RUNNER) $2
endef

define link_windows_xp_test
$1: $2
	mkdir -p $$(dir $$@)
	$$(WINDOWS_XP_CXX) $$(WINDOWS_XP_TEST_CXXFLAGS) $$(WINDOWS_XP_LDFLAGS) -o $$@ $$^ $$(WINDOWS_XP_LDLIBS)
endef

all-windows-xp: $(WINDOWS_XP_TARGET)

$(WINDOWS_XP_TARGET): $(WINDOWS_XP_OBJS)
	mkdir -p $(dir $@)
	$(WINDOWS_XP_CXX) $(WINDOWS_XP_PROD_CXXFLAGS) $(WINDOWS_XP_LDFLAGS) -o $@ $^ $(WINDOWS_XP_LDLIBS)

$(WINDOWS_XP_PROD_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(WINDOWS_XP_CXX) $(WINDOWS_XP_PROD_CPPFLAGS) $(WINDOWS_XP_PROD_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(WINDOWS_XP_TEST_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(WINDOWS_XP_CXX) $(WINDOWS_XP_TEST_CPPFLAGS) $(WINDOWS_XP_TEST_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(eval $(call run_windows_xp_test,test-windows-xp-basic-mutex,$(XP_BASIC_MUTEX)))
$(eval $(call link_windows_xp_test,$(XP_BASIC_MUTEX),$(XP_BASIC_MUTEX_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-patch,$(XP_PATCH)))
$(eval $(call link_windows_xp_test,$(XP_PATCH),$(XP_PATCH_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-session-store,$(XP_SESSION_STORE)))
$(eval $(call link_windows_xp_test,$(XP_SESSION_STORE),$(XP_SESSION_STORE_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-server-streaming,$(XP_SERVER_STREAMING)))
$(eval $(call link_windows_xp_test,$(XP_SERVER_STREAMING),$(XP_SERVER_STREAMING_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-transfer,$(XP_TRANSFER)))
$(eval $(call link_windows_xp_test,$(XP_TRANSFER),$(XP_TRANSFER_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-config,$(XP_CONFIG)))
$(eval $(call link_windows_xp_test,$(XP_CONFIG),$(XP_CONFIG_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-http-request,$(XP_HTTP_REQUEST)))
$(eval $(call link_windows_xp_test,$(XP_HTTP_REQUEST),$(XP_HTTP_REQUEST_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-server-transport,$(XP_SERVER_TRANSPORT)))
$(eval $(call link_windows_xp_test,$(XP_SERVER_TRANSPORT),$(XP_SERVER_TRANSPORT_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-connection-manager,$(XP_CONNECTION_MANAGER)))
$(eval $(call link_windows_xp_test,$(XP_CONNECTION_MANAGER),$(XP_CONNECTION_MANAGER_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-server-routes-common,$(XP_SERVER_ROUTES_COMMON)))
$(eval $(call link_windows_xp_test,$(XP_SERVER_ROUTES_COMMON),$(XP_SERVER_ROUTES_COMMON_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-server-routes,$(XP_SERVER_ROUTES)))
$(eval $(call link_windows_xp_test,$(XP_SERVER_ROUTES),$(XP_SERVER_ROUTES_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-server-runtime,$(XP_SERVER_RUNTIME)))
$(eval $(call link_windows_xp_test,$(XP_SERVER_RUNTIME),$(XP_SERVER_RUNTIME_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-sandbox,$(XP_SANDBOX)))
$(eval $(call link_windows_xp_test,$(XP_SANDBOX),$(XP_SANDBOX_OBJS)))

$(eval $(call run_windows_xp_test,test-windows-xp-port-tunnel-frame,$(XP_PORT_TUNNEL_FRAME)))
$(eval $(call link_windows_xp_test,$(XP_PORT_TUNNEL_FRAME),$(XP_PORT_TUNNEL_FRAME_OBJS)))

test-windows-xp: $(WINDOWS_XP_TEST_TARGETS)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_BASIC_MUTEX)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_PATCH)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_SESSION_STORE)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_SERVER_STREAMING)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_TRANSFER)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_CONFIG)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_HTTP_REQUEST)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_SERVER_TRANSPORT)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_CONNECTION_MANAGER)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_SERVER_ROUTES_COMMON)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_SERVER_ROUTES)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_SERVER_RUNTIME)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_SANDBOX)
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $(WINDOWS_XP_TEST_RUNNER) $(XP_PORT_TUNNEL_FRAME)

check-windows-xp: all-windows-xp test-windows-xp

.PHONY: all-windows-xp test-windows-xp test-windows-xp-basic-mutex test-windows-xp-patch test-windows-xp-session-store test-windows-xp-server-streaming test-windows-xp-transfer test-windows-xp-config test-windows-xp-http-request test-windows-xp-server-transport test-windows-xp-connection-manager test-windows-xp-server-routes-common test-windows-xp-server-routes test-windows-xp-server-runtime test-windows-xp-sandbox test-windows-xp-port-tunnel-frame check-windows-xp
