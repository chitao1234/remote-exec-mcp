WINDOWS_XP_CXX ?= i686-w64-mingw32-g++
WINE ?= wine

WINDOWS_XP_PROD_OBJ_DIR := $(OBJ_DIR)/windows-xp-prod
WINDOWS_XP_TEST_OBJ_DIR := $(OBJ_DIR)/windows-xp-test
WINDOWS_XP_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp-xp.exe

WINDOWS_XP_PROD_CPPFLAGS := $(COMMON_CPPFLAGS) -DWINVER=0x0501 -D_WIN32_WINNT=0x0501
WINDOWS_XP_PROD_CXXFLAGS := $(PROD_CXXFLAGS)
WINDOWS_XP_TEST_CPPFLAGS := $(COMMON_CPPFLAGS) -DWINVER=0x0501 -D_WIN32_WINNT=0x0501
WINDOWS_XP_TEST_CXXFLAGS := $(TEST_CXXFLAGS)
WINDOWS_XP_LDFLAGS := -static-libgcc -static-libstdc++
WINDOWS_XP_LDLIBS := -lws2_32

WINDOWS_XP_SRCS := \
	$(BASE_SRCS) \
	$(MAKEFILE_DIR)src/main.cpp \
	$(MAKEFILE_DIR)src/process_session_win32.cpp \
	$(MAKEFILE_DIR)src/console_output.cpp \
	$(MAKEFILE_DIR)src/win32_error.cpp

XP_SESSION_STORE := $(BUILD_DIR)/test_session_store-xp.exe
XP_TRANSFER := $(BUILD_DIR)/test_transfer-xp.exe

WINDOWS_XP_TEST_TARGETS := \
	$(XP_SESSION_STORE) \
	$(XP_TRANSFER)

XP_SESSION_STORE_SRCS := \
	$(MAKEFILE_DIR)tests/test_session_store.cpp \
	$(MAKEFILE_DIR)src/session_store.cpp \
	$(MAKEFILE_DIR)src/process_session_win32.cpp \
	$(MAKEFILE_DIR)src/platform.cpp \
	$(MAKEFILE_DIR)src/shell_policy.cpp \
	$(MAKEFILE_DIR)src/console_output.cpp \
	$(MAKEFILE_DIR)src/basic_mutex.cpp \
	$(MAKEFILE_DIR)src/logging.cpp \
	$(MAKEFILE_DIR)src/win32_error.cpp \
	$(MAKEFILE_DIR)src/config.cpp \
	$(MAKEFILE_DIR)src/text_utils.cpp

XP_TRANSFER_SRCS := \
	$(MAKEFILE_DIR)tests/test_transfer.cpp \
	$(PATH_UTILS_SRCS) \
	$(TRANSFER_SRCS) \
	$(RPC_FAILURE_SRCS)

WINDOWS_XP_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_PROD_OBJ_DIR),$(WINDOWS_XP_SRCS)))
XP_SESSION_STORE_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_SESSION_STORE_SRCS)))
XP_TRANSFER_OBJS := $(sort $(call cpp_objs,$(WINDOWS_XP_TEST_OBJ_DIR),$(XP_TRANSFER_SRCS)))

DEP_FILES += \
	$(WINDOWS_XP_OBJS:.o=.d) \
	$(XP_SESSION_STORE_OBJS:.o=.d) \
	$(XP_TRANSFER_OBJS:.o=.d)

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

test-wine-session-store: $(XP_SESSION_STORE)
	$(WINE) $(XP_SESSION_STORE)

$(XP_SESSION_STORE): $(XP_SESSION_STORE_OBJS)
	mkdir -p $(dir $@)
	$(WINDOWS_XP_CXX) $(WINDOWS_XP_TEST_CXXFLAGS) $(WINDOWS_XP_LDFLAGS) -o $@ $^ $(WINDOWS_XP_LDLIBS)

test-wine-transfer: $(XP_TRANSFER)
	$(WINE) $(XP_TRANSFER)

$(XP_TRANSFER): $(XP_TRANSFER_OBJS)
	mkdir -p $(dir $@)
	$(WINDOWS_XP_CXX) $(WINDOWS_XP_TEST_CXXFLAGS) $(WINDOWS_XP_LDFLAGS) -o $@ $^ $(WINDOWS_XP_LDLIBS)

check-windows-xp: all-windows-xp $(WINDOWS_XP_TEST_TARGETS)

.PHONY: all-windows-xp test-wine-session-store test-wine-transfer check-windows-xp
