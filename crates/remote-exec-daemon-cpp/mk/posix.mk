HOST_CXX ?= c++

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

$(HOST_PROD_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_PROD_CPPFLAGS) $(HOST_PROD_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(HOST_TEST_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(HOST_CXX) $(HOST_TEST_CPPFLAGS) $(HOST_TEST_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

$(foreach test,$(HOST_POSIX_TESTS),$(call register_host_test,$(test)))

test-server-streaming: $(HOST_SERVER_STREAMING_TEST_TARGET)

check-posix: $(HOST_TEST_PHONY_TARGETS) all-posix

.PHONY: all-posix $(HOST_TEST_PHONY_TARGETS) test-server-streaming check-posix
