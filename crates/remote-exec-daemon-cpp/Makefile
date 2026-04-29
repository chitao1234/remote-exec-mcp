CXX = i686-w64-mingw32-g++
HOST_CXX ?= g++
BUILD_DIR := build
POSIX_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp
WINDOWS_XP_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp-xp.exe
COMMON_FLAGS := -std=gnu++17 -O0 -Wall -Wextra -Iinclude -Ithird_party
WINDOWS_XP_FLAGS := $(COMMON_FLAGS) -DWINVER=0x0501 -D_WIN32_WINNT=0x0501 -static-libgcc -static-libstdc++
WIN_LIBS := -lws2_32
COMMON_SRCS := src/main.cpp src/config.cpp src/http_helpers.cpp src/http_request.cpp src/logging.cpp src/text_utils.cpp src/platform.cpp src/shell_policy.cpp src/server.cpp src/server_routes.cpp src/server_transport.cpp src/session_store.cpp src/patch_engine.cpp src/transfer_ops.cpp
WINDOWS_XP_SRCS := $(COMMON_SRCS) src/process_session_win32.cpp src/console_output.cpp src/win32_error.cpp
POSIX_SRCS := $(COMMON_SRCS) src/process_session_posix.cpp
HOST_PATCH := $(BUILD_DIR)/test_patch
HOST_TRANSFER := $(BUILD_DIR)/test_transfer
HOST_CONFIG := $(BUILD_DIR)/test_config
HOST_HTTP_REQUEST := $(BUILD_DIR)/test_http_request
HOST_SESSION_STORE := $(BUILD_DIR)/test_session_store
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
	./$(HOST_PATCH)

$(HOST_PATCH): tests/test_patch.cpp src/patch_engine.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ tests/test_patch.cpp src/patch_engine.cpp

test-host-transfer: $(HOST_TRANSFER)
	./$(HOST_TRANSFER)

$(HOST_TRANSFER): tests/test_transfer.cpp src/transfer_ops.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ tests/test_transfer.cpp src/transfer_ops.cpp

test-host-config: $(HOST_CONFIG)
	./$(HOST_CONFIG)

$(HOST_CONFIG): tests/test_config.cpp src/config.cpp src/text_utils.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ tests/test_config.cpp src/config.cpp src/text_utils.cpp

test-host-http-request: $(HOST_HTTP_REQUEST)
	./$(HOST_HTTP_REQUEST)

$(HOST_HTTP_REQUEST): tests/test_http_request.cpp src/http_request.cpp src/http_helpers.cpp src/text_utils.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ tests/test_http_request.cpp src/http_request.cpp src/http_helpers.cpp src/text_utils.cpp

test-host-session-store: $(HOST_SESSION_STORE)
	./$(HOST_SESSION_STORE)

$(HOST_SESSION_STORE): tests/test_session_store.cpp src/session_store.cpp src/process_session_posix.cpp src/platform.cpp src/shell_policy.cpp src/logging.cpp src/config.cpp src/text_utils.cpp
	mkdir -p $(BUILD_DIR)
	$(HOST_CXX) $(COMMON_FLAGS) -o $@ tests/test_session_store.cpp src/session_store.cpp src/process_session_posix.cpp src/platform.cpp src/shell_policy.cpp src/logging.cpp src/config.cpp src/text_utils.cpp

test-wine-session-store: $(XP_SESSION_STORE)
	$(WINE) ./$(XP_SESSION_STORE)

$(XP_SESSION_STORE): tests/test_session_store.cpp src/session_store.cpp src/process_session_win32.cpp src/platform.cpp src/shell_policy.cpp src/console_output.cpp src/logging.cpp src/win32_error.cpp src/config.cpp src/text_utils.cpp
	mkdir -p $(BUILD_DIR)
	$(CXX) $(WINDOWS_XP_FLAGS) -o $@ tests/test_session_store.cpp src/session_store.cpp src/process_session_win32.cpp src/platform.cpp src/shell_policy.cpp src/console_output.cpp src/logging.cpp src/win32_error.cpp src/config.cpp src/text_utils.cpp $(WIN_LIBS)

check-posix: test-host-patch test-host-transfer test-host-config test-host-http-request test-host-session-store all-posix

check-windows-xp: all-windows-xp

check: check-posix

.PHONY: all all-posix all-windows-xp clean test-host-patch test-host-transfer test-host-config test-host-http-request test-host-session-store test-wine-session-store check-posix check-windows-xp check
