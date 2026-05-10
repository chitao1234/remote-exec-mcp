HOST_CXX ?= g++

HOST_PROD_OBJ_DIR := $(OBJ_DIR)/host-prod
HOST_TEST_OBJ_DIR := $(OBJ_DIR)/host-test
POSIX_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp

# POSIX builds use pthread APIs. Use the compiler-driver flag, not -lpthread, so
# both compilation and linking get the platform's thread-aware settings.
POSIX_PTHREAD_FLAGS := -pthread
HOST_PROD_CPPFLAGS := $(COMMON_CPPFLAGS)
HOST_PROD_CXXFLAGS := $(PROD_CXXFLAGS) $(POSIX_PTHREAD_FLAGS)
HOST_PROD_LDFLAGS ?=
HOST_PROD_LDLIBS := $(POSIX_PTHREAD_FLAGS)
HOST_TEST_CPPFLAGS := $(COMMON_CPPFLAGS) -DREMOTE_EXEC_CPP_TESTING
HOST_TEST_CXXFLAGS := $(TEST_CXXFLAGS) $(POSIX_PTHREAD_FLAGS)
HOST_TEST_LDFLAGS ?=
HOST_TEST_LDLIBS := $(POSIX_PTHREAD_FLAGS)

HOST_PATCH := $(BUILD_DIR)/test_patch
HOST_TRANSFER := $(BUILD_DIR)/test_transfer
HOST_CONFIG := $(BUILD_DIR)/test_config
HOST_BASIC_MUTEX := $(BUILD_DIR)/test_basic_mutex
HOST_HTTP_REQUEST := $(BUILD_DIR)/test_http_request
HOST_SERVER_TRANSPORT := $(BUILD_DIR)/test_server_transport
HOST_SERVER_STREAMING := $(BUILD_DIR)/test_server_streaming
HOST_SESSION_STORE := $(BUILD_DIR)/test_session_store
HOST_CONNECTION_MANAGER := $(BUILD_DIR)/test_connection_manager
HOST_SERVER_RUNTIME := $(BUILD_DIR)/test_server_runtime
HOST_SERVER_ROUTES := $(BUILD_DIR)/test_server_routes
HOST_SANDBOX := $(BUILD_DIR)/test_sandbox
HOST_PORT_TUNNEL_FRAME := $(BUILD_DIR)/test_port_tunnel_frame

HOST_TEST_PHONY_TARGETS := \
	test-host-basic-mutex \
	test-host-patch \
	test-host-transfer \
	test-host-config \
	test-host-http-request \
	test-host-server-transport \
	test-host-server-streaming \
	test-host-session-store \
	test-host-connection-manager \
	test-host-server-runtime \
	test-host-server-routes \
	test-host-sandbox \
	test-port-tunnel-frame

POSIX_OBJS := $(sort $(call cpp_objs,$(HOST_PROD_OBJ_DIR),$(POSIX_SRCS)))
HOST_PATCH_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_PATCH_SRCS)))
HOST_TRANSFER_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_TRANSFER_SRCS)))
HOST_CONFIG_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_CONFIG_SRCS)))
HOST_BASIC_MUTEX_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_BASIC_MUTEX_SRCS)))
HOST_HTTP_REQUEST_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_HTTP_REQUEST_SRCS)))
HOST_SERVER_TRANSPORT_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_SERVER_TRANSPORT_SRCS)))
HOST_SERVER_STREAMING_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_SERVER_STREAMING_SRCS)))
HOST_SESSION_STORE_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_SESSION_STORE_SRCS)))
HOST_CONNECTION_MANAGER_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_CONNECTION_MANAGER_SRCS)))
HOST_SERVER_RUNTIME_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_SERVER_RUNTIME_SRCS)))
HOST_SERVER_ROUTES_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_SERVER_ROUTES_SRCS)))
HOST_SANDBOX_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_SANDBOX_SRCS)))
HOST_PORT_TUNNEL_FRAME_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_PORT_TUNNEL_FRAME_SRCS)))

DEP_FILES += \
	$(POSIX_OBJS:.o=.d) \
	$(HOST_PATCH_OBJS:.o=.d) \
	$(HOST_TRANSFER_OBJS:.o=.d) \
	$(HOST_CONFIG_OBJS:.o=.d) \
	$(HOST_BASIC_MUTEX_OBJS:.o=.d) \
	$(HOST_HTTP_REQUEST_OBJS:.o=.d) \
	$(HOST_SERVER_TRANSPORT_OBJS:.o=.d) \
	$(HOST_SERVER_STREAMING_OBJS:.o=.d) \
	$(HOST_SESSION_STORE_OBJS:.o=.d) \
	$(HOST_CONNECTION_MANAGER_OBJS:.o=.d) \
	$(HOST_SERVER_RUNTIME_OBJS:.o=.d) \
	$(HOST_SERVER_ROUTES_OBJS:.o=.d) \
	$(HOST_SANDBOX_OBJS:.o=.d) \
	$(HOST_PORT_TUNNEL_FRAME_OBJS:.o=.d)

define link_host_test
$1: $2
	mkdir -p $$(dir $$@)
	$$(HOST_CXX) $$(HOST_TEST_CXXFLAGS) $$(HOST_TEST_LDFLAGS) -o $$@ $$^ $$(HOST_TEST_LDLIBS)
endef

all-posix: $(POSIX_TARGET)

$(POSIX_TARGET): $(POSIX_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_PROD_CXXFLAGS) $(HOST_PROD_LDFLAGS) -o $@ $^ $(HOST_PROD_LDLIBS)

$(HOST_PROD_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_PROD_CPPFLAGS) $(HOST_PROD_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(HOST_TEST_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CPPFLAGS) $(HOST_TEST_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(eval $(call run_test,test-host-patch,$(HOST_PATCH)))
$(eval $(call link_host_test,$(HOST_PATCH),$(HOST_PATCH_OBJS)))

$(eval $(call run_test,test-host-transfer,$(HOST_TRANSFER)))
$(eval $(call link_host_test,$(HOST_TRANSFER),$(HOST_TRANSFER_OBJS)))

$(eval $(call run_test,test-host-config,$(HOST_CONFIG)))
$(eval $(call link_host_test,$(HOST_CONFIG),$(HOST_CONFIG_OBJS)))

$(eval $(call run_test,test-host-basic-mutex,$(HOST_BASIC_MUTEX)))
$(eval $(call link_host_test,$(HOST_BASIC_MUTEX),$(HOST_BASIC_MUTEX_OBJS)))

$(eval $(call run_test,test-host-http-request,$(HOST_HTTP_REQUEST)))
$(eval $(call link_host_test,$(HOST_HTTP_REQUEST),$(HOST_HTTP_REQUEST_OBJS)))

$(eval $(call run_test,test-host-server-transport,$(HOST_SERVER_TRANSPORT)))
$(eval $(call link_host_test,$(HOST_SERVER_TRANSPORT),$(HOST_SERVER_TRANSPORT_OBJS)))

$(eval $(call run_test,test-host-server-streaming,$(HOST_SERVER_STREAMING)))
test-server-streaming: test-host-server-streaming
$(eval $(call link_host_test,$(HOST_SERVER_STREAMING),$(HOST_SERVER_STREAMING_OBJS)))

$(eval $(call run_test,test-host-session-store,$(HOST_SESSION_STORE)))
$(eval $(call link_host_test,$(HOST_SESSION_STORE),$(HOST_SESSION_STORE_OBJS)))

$(eval $(call run_test,test-host-connection-manager,$(HOST_CONNECTION_MANAGER)))
$(eval $(call link_host_test,$(HOST_CONNECTION_MANAGER),$(HOST_CONNECTION_MANAGER_OBJS)))

$(eval $(call run_test,test-host-server-runtime,$(HOST_SERVER_RUNTIME)))
$(eval $(call link_host_test,$(HOST_SERVER_RUNTIME),$(HOST_SERVER_RUNTIME_OBJS)))

$(eval $(call run_test,test-host-server-routes,$(HOST_SERVER_ROUTES)))
$(eval $(call link_host_test,$(HOST_SERVER_ROUTES),$(HOST_SERVER_ROUTES_OBJS)))

$(eval $(call run_test,test-host-sandbox,$(HOST_SANDBOX)))
$(eval $(call link_host_test,$(HOST_SANDBOX),$(HOST_SANDBOX_OBJS)))

$(eval $(call run_test,test-port-tunnel-frame,$(HOST_PORT_TUNNEL_FRAME)))
$(eval $(call link_host_test,$(HOST_PORT_TUNNEL_FRAME),$(HOST_PORT_TUNNEL_FRAME_OBJS)))

check-posix: $(HOST_TEST_PHONY_TARGETS) all-posix

.PHONY: all-posix $(HOST_TEST_PHONY_TARGETS) test-server-streaming check-posix
