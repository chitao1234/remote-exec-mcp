CXX = i686-w64-mingw32-g++
HOST_CXX ?= g++
MAKEFILE_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
BUILD_DIR ?= $(MAKEFILE_DIR)build
POSIX_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp
WINDOWS_XP_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp-xp.exe
COMMON_FLAGS := -std=gnu++17 -O0 -Wall -Wextra -I$(MAKEFILE_DIR)include -I$(MAKEFILE_DIR)third_party
WINDOWS_XP_FLAGS := $(COMMON_FLAGS) -DWINVER=0x0501 -D_WIN32_WINNT=0x0501 -static-libgcc -static-libstdc++
WIN_LIBS := -lws2_32
COMMON_SRCS := $(addprefix $(MAKEFILE_DIR),src/main.cpp src/config.cpp src/http_helpers.cpp src/http_request.cpp src/logging.cpp src/text_utils.cpp src/platform.cpp src/shell_policy.cpp src/server.cpp src/server_routes.cpp src/server_transport.cpp src/session_store.cpp src/patch_engine.cpp src/transfer_ops.cpp)
WINDOWS_XP_SRCS := $(COMMON_SRCS) $(addprefix $(MAKEFILE_DIR),src/process_session_win32.cpp src/console_output.cpp src/win32_error.cpp)
POSIX_SRCS := $(COMMON_SRCS) $(MAKEFILE_DIR)src/process_session_posix.cpp
HOST_PATCH := $(BUILD_DIR)/test_patch
HOST_TRANSFER := $(BUILD_DIR)/test_transfer
HOST_CONFIG := $(BUILD_DIR)/test_config
HOST_HTTP_REQUEST := $(BUILD_DIR)/test_http_request
HOST_SESSION_STORE := $(BUILD_DIR)/test_session_store
HOST_SERVER_ROUTES := $(BUILD_DIR)/test_server_routes
XP_SESSION_STORE := $(BUILD_DIR)/test_session_store-xp.exe
WINE ?= wine

all: all-posix

all-posix: $(POSIX_TARGET)

all-windows-xp: $(WINDOWS_XP_TARGET)

$(POSIX_TARGET): $(POSIX_SRCS)
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ $(POSIX_SRCS)

$(WINDOWS_XP_TARGET): $(WINDOWS_XP_SRCS)
	mkdir -p $(BUILD_DIR)
	$(CXX) $(WINDOWS_XP_FLAGS) -o $@ $(WINDOWS_XP_SRCS) $(WIN_LIBS)

clean:
	rm -rf $(BUILD_DIR)

test-host-patch: $(HOST_PATCH)
	$(HOST_PATCH)

$(HOST_PATCH): $(MAKEFILE_DIR)tests/test_patch.cpp $(MAKEFILE_DIR)src/patch_engine.cpp $(MAKEFILE_DIR)src/platform.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ $^

test-host-transfer: $(HOST_TRANSFER)
	$(HOST_TRANSFER)

$(HOST_TRANSFER): $(MAKEFILE_DIR)tests/test_transfer.cpp $(MAKEFILE_DIR)src/transfer_ops.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ $^

test-host-config: $(HOST_CONFIG)
	$(HOST_CONFIG)

$(HOST_CONFIG): $(MAKEFILE_DIR)tests/test_config.cpp $(MAKEFILE_DIR)src/config.cpp $(MAKEFILE_DIR)src/text_utils.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ $^

test-host-http-request: $(HOST_HTTP_REQUEST)
	$(HOST_HTTP_REQUEST)

$(HOST_HTTP_REQUEST): $(MAKEFILE_DIR)tests/test_http_request.cpp $(MAKEFILE_DIR)src/http_request.cpp $(MAKEFILE_DIR)src/http_helpers.cpp $(MAKEFILE_DIR)src/text_utils.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ $^

test-host-session-store: $(HOST_SESSION_STORE)
	$(HOST_SESSION_STORE)

$(HOST_SESSION_STORE): $(MAKEFILE_DIR)tests/test_session_store.cpp $(MAKEFILE_DIR)src/session_store.cpp $(MAKEFILE_DIR)src/process_session_posix.cpp $(MAKEFILE_DIR)src/platform.cpp $(MAKEFILE_DIR)src/shell_policy.cpp $(MAKEFILE_DIR)src/logging.cpp $(MAKEFILE_DIR)src/config.cpp $(MAKEFILE_DIR)src/text_utils.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ $^

test-host-server-routes: $(HOST_SERVER_ROUTES)
	$(HOST_SERVER_ROUTES)

$(HOST_SERVER_ROUTES): $(MAKEFILE_DIR)tests/test_server_routes.cpp $(MAKEFILE_DIR)src/server_routes.cpp $(MAKEFILE_DIR)src/http_helpers.cpp $(MAKEFILE_DIR)src/session_store.cpp $(MAKEFILE_DIR)src/process_session_posix.cpp $(MAKEFILE_DIR)src/platform.cpp $(MAKEFILE_DIR)src/shell_policy.cpp $(MAKEFILE_DIR)src/patch_engine.cpp $(MAKEFILE_DIR)src/transfer_ops.cpp $(MAKEFILE_DIR)src/logging.cpp $(MAKEFILE_DIR)src/config.cpp $(MAKEFILE_DIR)src/text_utils.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ $^

test-wine-session-store: $(XP_SESSION_STORE)
	$(WINE) $(XP_SESSION_STORE)

$(XP_SESSION_STORE): $(MAKEFILE_DIR)tests/test_session_store.cpp $(MAKEFILE_DIR)src/session_store.cpp $(MAKEFILE_DIR)src/process_session_win32.cpp $(MAKEFILE_DIR)src/platform.cpp $(MAKEFILE_DIR)src/shell_policy.cpp $(MAKEFILE_DIR)src/console_output.cpp $(MAKEFILE_DIR)src/logging.cpp $(MAKEFILE_DIR)src/win32_error.cpp $(MAKEFILE_DIR)src/config.cpp $(MAKEFILE_DIR)src/text_utils.cpp
	mkdir -p $(BUILD_DIR)
	$(CXX) $(WINDOWS_XP_FLAGS) -o $@ $^ $(WIN_LIBS)

check-posix: test-host-patch test-host-transfer test-host-config test-host-http-request test-host-session-store test-host-server-routes all-posix

check-windows-xp: all-windows-xp

check: check-posix

.PHONY: all all-posix all-windows-xp clean test-host-patch test-host-transfer test-host-config test-host-http-request test-host-session-store test-host-server-routes test-wine-session-store check-posix check-windows-xp check
