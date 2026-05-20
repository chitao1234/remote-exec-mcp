HOST_CXX ?= c++
STRESS_RUNS ?= 10
STRESS_JOBS ?= 8

HOST_PROD_OBJ_DIR := $(OBJ_DIR)/host-prod
HOST_TEST_OBJ_DIR := $(OBJ_DIR)/host-test
POSIX_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp
POSIX_CONFIG_HEADER := $(BUILD_DIR)/generated/remote_exec_cpp_config.h
POSIX_CONFIG_SCRIPT := $(MAKEFILE_DIR)scripts/write_posix_config_header.sh
POSIX_SOCKET_LDLIBS_SCRIPT := $(MAKEFILE_DIR)scripts/print_posix_socket_ldlibs.sh

# POSIX builds use pthread APIs. Use the compiler-driver flag, not -lpthread, so
# both compilation and linking get the platform's thread-aware settings.
POSIX_PTHREAD_FLAGS := -pthread
POSIX_BASE_LDLIBS := $(POSIX_PTHREAD_FLAGS)
HOST_PROD_CPPFLAGS := -I$(BUILD_DIR)/generated $(COMMON_CPPFLAGS)
HOST_PROD_CXXFLAGS := $(PROD_CXXFLAGS) $(POSIX_PTHREAD_FLAGS)
HOST_PROD_LDFLAGS ?=
HOST_TEST_CPPFLAGS := -I$(BUILD_DIR)/generated $(COMMON_CPPFLAGS) -DREMOTE_EXEC_CPP_TESTING
HOST_TEST_CXXFLAGS := $(TEST_CXXFLAGS) $(POSIX_PTHREAD_FLAGS)
HOST_TEST_LDFLAGS ?=

POSIX_SOCKET_LDLIBS ?= $(shell HOST_CXX="$(HOST_CXX)" PROBE_CPPFLAGS="$(COMMON_CPPFLAGS)" PROBE_CXXFLAGS="$(PROD_CXXFLAGS) $(POSIX_PTHREAD_FLAGS)" PROBE_LDFLAGS="$(HOST_PROD_LDFLAGS)" PROBE_LDLIBS="$(POSIX_BASE_LDLIBS)" sh "$(POSIX_SOCKET_LDLIBS_SCRIPT)")
HOST_PROD_LDLIBS := $(POSIX_BASE_LDLIBS) $(POSIX_SOCKET_LDLIBS)
HOST_TEST_LDLIBS := $(POSIX_BASE_LDLIBS) $(POSIX_SOCKET_LDLIBS)

include $(MAKEFILE_DIR)mk/host-tests.mk

$(foreach test,$(HOST_POSIX_TESTS),$(eval HOST_$(test) := $(BUILD_DIR)/$(HOST_$(test)_BIN)))

HOST_TEST_PHONY_TARGETS := $(foreach test,$(HOST_POSIX_TESTS),$(HOST_$(test)_TEST_TARGET))

POSIX_OBJS := $(sort $(call cpp_objs,$(HOST_PROD_OBJ_DIR),$(POSIX_SRCS)))
$(foreach test,$(HOST_POSIX_TESTS),$(eval HOST_$(test)_OBJS := $(sort $(call cpp_objs,$(HOST_TEST_OBJ_DIR),$(HOST_$(test)_SRCS)))))

DEP_FILES += $(POSIX_OBJS:.o=.d)
$(foreach test,$(HOST_POSIX_TESTS),$(eval DEP_FILES += $(patsubst %.o,%.d,$(HOST_$(test)_OBJS))))

define link_host_test
$1: $2
	mkdir -p $$(dir $$@)
	$$(HOST_CXX) $$(HOST_TEST_CXXFLAGS) $$(HOST_TEST_LDFLAGS) -o $$@ $$^ $$(HOST_TEST_LDLIBS)
endef

define register_host_test
$(eval $(call run_test,$(HOST_$(1)_TEST_TARGET),$(HOST_$(1))))
$(eval $(call link_host_test,$(HOST_$(1)),$(HOST_$(1)_OBJS)))
endef

all-posix: $(POSIX_TARGET)

$(POSIX_TARGET): $(POSIX_OBJS)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_PROD_CXXFLAGS) $(HOST_PROD_LDFLAGS) -o $@ $^ $(HOST_PROD_LDLIBS)

$(POSIX_CONFIG_HEADER): $(POSIX_CONFIG_SCRIPT) force-posix-config
	mkdir -p $(dir $@)
	HOST_CXX="$(HOST_CXX)" \
	    PROBE_CPPFLAGS="$(COMMON_CPPFLAGS)" \
	    PROBE_CXXFLAGS="$(PROD_CXXFLAGS) $(POSIX_PTHREAD_FLAGS)" \
	    PROBE_LDFLAGS="$(HOST_PROD_LDFLAGS)" \
	    PROBE_LDLIBS="$(HOST_PROD_LDLIBS)" \
	    sh $< $@

$(HOST_PROD_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp $(POSIX_CONFIG_HEADER)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_PROD_CPPFLAGS) $(HOST_PROD_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(HOST_TEST_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp $(POSIX_CONFIG_HEADER)
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CPPFLAGS) $(HOST_TEST_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(foreach test,$(HOST_POSIX_TESTS),$(call register_host_test,$(test)))

test-server-streaming: $(HOST_SERVER_STREAMING_TEST_TARGET)

check-posix: $(HOST_TEST_PHONY_TARGETS) all-posix

force-posix-config:

stress-posix:
	@i=1; \
	while [ $$i -le $(STRESS_RUNS) ]; do \
		echo "stress-posix iteration $$i/$(STRESS_RUNS)"; \
		$(MAKE) check-posix -j $(STRESS_JOBS) || exit $$?; \
		i=$$((i + 1)); \
	done

.PHONY: all-posix $(HOST_TEST_PHONY_TARGETS) test-server-streaming check-posix force-posix-config stress-posix
