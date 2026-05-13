WINDOWS_NATIVE_CXX ?= g++

WINDOWS_NATIVE_PROD_OBJ_DIR := $(OBJ_DIR)/windows-native-prod
WINDOWS_NATIVE_TEST_OBJ_DIR := $(OBJ_DIR)/windows-native-test
WINDOWS_NATIVE_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp.exe
WINDOWS_NATIVE_BASIC_MUTEX := $(BUILD_DIR)/test_basic_mutex.exe
WINDOWS_NATIVE_PATCH := $(BUILD_DIR)/test_patch.exe
WINDOWS_NATIVE_SESSION_STORE := $(BUILD_DIR)/test_session_store.exe
WINDOWS_NATIVE_TRANSFER := $(BUILD_DIR)/test_transfer.exe
WINDOWS_NATIVE_CONFIG := $(BUILD_DIR)/test_config.exe
WINDOWS_NATIVE_HTTP_REQUEST := $(BUILD_DIR)/test_http_request.exe
WINDOWS_NATIVE_SERVER_TRANSPORT := $(BUILD_DIR)/test_server_transport.exe
WINDOWS_NATIVE_CONNECTION_MANAGER := $(BUILD_DIR)/test_connection_manager.exe
WINDOWS_NATIVE_SERVER_ROUTES_COMMON := $(BUILD_DIR)/test_server_routes_common.exe
WINDOWS_NATIVE_SERVER_RUNTIME := $(BUILD_DIR)/test_server_runtime.exe
WINDOWS_NATIVE_SANDBOX := $(BUILD_DIR)/test_sandbox.exe
WINDOWS_NATIVE_PORT_TUNNEL_FRAME := $(BUILD_DIR)/test_port_tunnel_frame.exe

WINDOWS_NATIVE_PROD_CPPFLAGS := $(COMMON_CPPFLAGS) -DWIN32_LEAN_AND_MEAN
WINDOWS_NATIVE_PROD_CXXFLAGS := $(PROD_CXXFLAGS)
WINDOWS_NATIVE_TEST_CPPFLAGS := $(COMMON_CPPFLAGS) -DWIN32_LEAN_AND_MEAN -DREMOTE_EXEC_CPP_TESTING
WINDOWS_NATIVE_TEST_CXXFLAGS := $(TEST_CXXFLAGS)
WINDOWS_NATIVE_LDFLAGS ?=
WINDOWS_NATIVE_LDLIBS := -lws2_32

WINDOWS_NATIVE_SRCS := $(WINDOWS_DAEMON_SRCS)

WINDOWS_NATIVE_PLATFORM_NEUTRAL_TEST_TARGETS := \
	$(WINDOWS_NATIVE_BASIC_MUTEX) \
	$(WINDOWS_NATIVE_PATCH) \
	$(WINDOWS_NATIVE_TRANSFER) \
	$(WINDOWS_NATIVE_CONFIG) \
	$(WINDOWS_NATIVE_HTTP_REQUEST) \
	$(WINDOWS_NATIVE_SERVER_TRANSPORT) \
	$(WINDOWS_NATIVE_CONNECTION_MANAGER) \
	$(WINDOWS_NATIVE_SERVER_ROUTES_COMMON) \
	$(WINDOWS_NATIVE_SANDBOX) \
	$(WINDOWS_NATIVE_PORT_TUNNEL_FRAME)

WINDOWS_NATIVE_PROCESS_TEST_TARGETS := $(WINDOWS_NATIVE_SESSION_STORE)

WINDOWS_NATIVE_SERVER_SMOKE_TEST_TARGETS := $(WINDOWS_NATIVE_SERVER_RUNTIME)

WINDOWS_NATIVE_TEST_TARGETS := \
	$(WINDOWS_NATIVE_PLATFORM_NEUTRAL_TEST_TARGETS) \
	$(WINDOWS_NATIVE_PROCESS_TEST_TARGETS) \
	$(WINDOWS_NATIVE_SERVER_SMOKE_TEST_TARGETS)

WINDOWS_NATIVE_BASIC_MUTEX_SRCS := $(HOST_BASIC_MUTEX_SRCS)
WINDOWS_NATIVE_PATCH_SRCS := $(HOST_PATCH_SRCS)
WINDOWS_NATIVE_SESSION_STORE_SRCS := $(WINDOWS_SESSION_STORE_TEST_SRCS)
WINDOWS_NATIVE_TRANSFER_SRCS := $(HOST_TRANSFER_SRCS)
WINDOWS_NATIVE_CONFIG_SRCS := $(HOST_CONFIG_SRCS)
WINDOWS_NATIVE_HTTP_REQUEST_SRCS := $(HOST_HTTP_REQUEST_SRCS)
WINDOWS_NATIVE_SERVER_TRANSPORT_SRCS := $(HOST_SERVER_TRANSPORT_SRCS)
WINDOWS_NATIVE_CONNECTION_MANAGER_SRCS := $(HOST_CONNECTION_MANAGER_SRCS)
WINDOWS_NATIVE_SERVER_ROUTES_COMMON_SRCS := $(WINDOWS_SERVER_ROUTES_COMMON_TEST_SRCS)
WINDOWS_NATIVE_SERVER_RUNTIME_SRCS := $(WINDOWS_SERVER_RUNTIME_TEST_SRCS)
WINDOWS_NATIVE_SANDBOX_SRCS := $(HOST_SANDBOX_SRCS)
WINDOWS_NATIVE_PORT_TUNNEL_FRAME_SRCS := $(HOST_PORT_TUNNEL_FRAME_SRCS)

WINDOWS_NATIVE_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_PROD_OBJ_DIR),$(WINDOWS_NATIVE_SRCS)))
WINDOWS_NATIVE_BASIC_MUTEX_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_BASIC_MUTEX_SRCS)))
WINDOWS_NATIVE_PATCH_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_PATCH_SRCS)))
WINDOWS_NATIVE_SESSION_STORE_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_SESSION_STORE_SRCS)))
WINDOWS_NATIVE_TRANSFER_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_TRANSFER_SRCS)))
WINDOWS_NATIVE_CONFIG_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_CONFIG_SRCS)))
WINDOWS_NATIVE_HTTP_REQUEST_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_HTTP_REQUEST_SRCS)))
WINDOWS_NATIVE_SERVER_TRANSPORT_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_SERVER_TRANSPORT_SRCS)))
WINDOWS_NATIVE_CONNECTION_MANAGER_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_CONNECTION_MANAGER_SRCS)))
WINDOWS_NATIVE_SERVER_ROUTES_COMMON_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_SERVER_ROUTES_COMMON_SRCS)))
WINDOWS_NATIVE_SERVER_RUNTIME_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_SERVER_RUNTIME_SRCS)))
WINDOWS_NATIVE_SANDBOX_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_SANDBOX_SRCS)))
WINDOWS_NATIVE_PORT_TUNNEL_FRAME_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_TEST_OBJ_DIR),$(WINDOWS_NATIVE_PORT_TUNNEL_FRAME_SRCS)))

DEP_FILES += \
	$(WINDOWS_NATIVE_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_BASIC_MUTEX_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_PATCH_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_SESSION_STORE_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_TRANSFER_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_CONFIG_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_HTTP_REQUEST_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_SERVER_TRANSPORT_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_CONNECTION_MANAGER_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_SERVER_ROUTES_COMMON_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_SERVER_RUNTIME_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_SANDBOX_OBJS:.o=.d) \
	$(WINDOWS_NATIVE_PORT_TUNNEL_FRAME_OBJS:.o=.d)

define link_windows_native_test
$1: $2
	mkdir -p $$(dir $$@)
	$$(WINDOWS_NATIVE_CXX) $$(WINDOWS_NATIVE_TEST_CXXFLAGS) $$(WINDOWS_NATIVE_LDFLAGS) -o $$@ $$^ $$(WINDOWS_NATIVE_LDLIBS)
endef

all-windows-native: $(WINDOWS_NATIVE_TARGET)

$(WINDOWS_NATIVE_TARGET): $(WINDOWS_NATIVE_OBJS)
	mkdir -p $(dir $@)
	$(WINDOWS_NATIVE_CXX) $(WINDOWS_NATIVE_PROD_CXXFLAGS) $(WINDOWS_NATIVE_LDFLAGS) -o $@ $^ $(WINDOWS_NATIVE_LDLIBS)

$(WINDOWS_NATIVE_PROD_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(WINDOWS_NATIVE_CXX) $(WINDOWS_NATIVE_PROD_CPPFLAGS) $(WINDOWS_NATIVE_PROD_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(WINDOWS_NATIVE_TEST_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(WINDOWS_NATIVE_CXX) $(WINDOWS_NATIVE_TEST_CPPFLAGS) $(WINDOWS_NATIVE_TEST_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(eval $(call run_test,test-windows-native-basic-mutex,$(WINDOWS_NATIVE_BASIC_MUTEX)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_BASIC_MUTEX),$(WINDOWS_NATIVE_BASIC_MUTEX_OBJS)))

$(eval $(call run_test,test-windows-native-patch,$(WINDOWS_NATIVE_PATCH)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_PATCH),$(WINDOWS_NATIVE_PATCH_OBJS)))

$(eval $(call run_test,test-windows-native-session-store,$(WINDOWS_NATIVE_SESSION_STORE)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_SESSION_STORE),$(WINDOWS_NATIVE_SESSION_STORE_OBJS)))

$(eval $(call run_test,test-windows-native-transfer,$(WINDOWS_NATIVE_TRANSFER)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_TRANSFER),$(WINDOWS_NATIVE_TRANSFER_OBJS)))

$(eval $(call run_test,test-windows-native-config,$(WINDOWS_NATIVE_CONFIG)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_CONFIG),$(WINDOWS_NATIVE_CONFIG_OBJS)))

$(eval $(call run_test,test-windows-native-http-request,$(WINDOWS_NATIVE_HTTP_REQUEST)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_HTTP_REQUEST),$(WINDOWS_NATIVE_HTTP_REQUEST_OBJS)))

$(eval $(call run_test,test-windows-native-server-transport,$(WINDOWS_NATIVE_SERVER_TRANSPORT)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_SERVER_TRANSPORT),$(WINDOWS_NATIVE_SERVER_TRANSPORT_OBJS)))

$(eval $(call run_test,test-windows-native-connection-manager,$(WINDOWS_NATIVE_CONNECTION_MANAGER)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_CONNECTION_MANAGER),$(WINDOWS_NATIVE_CONNECTION_MANAGER_OBJS)))

$(eval $(call run_test,test-windows-native-server-routes-common,$(WINDOWS_NATIVE_SERVER_ROUTES_COMMON)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_SERVER_ROUTES_COMMON),$(WINDOWS_NATIVE_SERVER_ROUTES_COMMON_OBJS)))

$(eval $(call run_test,test-windows-native-server-runtime,$(WINDOWS_NATIVE_SERVER_RUNTIME)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_SERVER_RUNTIME),$(WINDOWS_NATIVE_SERVER_RUNTIME_OBJS)))

$(eval $(call run_test,test-windows-native-sandbox,$(WINDOWS_NATIVE_SANDBOX)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_SANDBOX),$(WINDOWS_NATIVE_SANDBOX_OBJS)))

$(eval $(call run_test,test-windows-native-port-tunnel-frame,$(WINDOWS_NATIVE_PORT_TUNNEL_FRAME)))
$(eval $(call link_windows_native_test,$(WINDOWS_NATIVE_PORT_TUNNEL_FRAME),$(WINDOWS_NATIVE_PORT_TUNNEL_FRAME_OBJS)))

test-windows-native: \
	test-windows-native-basic-mutex \
	test-windows-native-patch \
	test-windows-native-session-store \
	test-windows-native-transfer \
	test-windows-native-config \
	test-windows-native-http-request \
	test-windows-native-server-transport \
	test-windows-native-connection-manager \
	test-windows-native-server-routes-common \
	test-windows-native-server-runtime \
	test-windows-native-sandbox \
	test-windows-native-port-tunnel-frame

check-windows-native: all-windows-native test-windows-native

.PHONY: \
	all-windows-native \
	test-windows-native \
	test-windows-native-basic-mutex \
	test-windows-native-patch \
	test-windows-native-session-store \
	test-windows-native-transfer \
	test-windows-native-config \
	test-windows-native-http-request \
	test-windows-native-server-transport \
	test-windows-native-connection-manager \
	test-windows-native-server-routes-common \
	test-windows-native-server-runtime \
	test-windows-native-sandbox \
	test-windows-native-port-tunnel-frame \
	check-windows-native
