CXX = i686-w64-mingw32-g++
HOST_CXX ?= g++
BUILD_DIR := build
TARGET := $(BUILD_DIR)/remote-exec-daemon-xp.exe
COMMON_FLAGS := -std=gnu++17 -O0 -Wall -Wextra -Iinclude -Ithird_party
TARGET_FLAGS := $(COMMON_FLAGS) -DWINVER=0x0501 -D_WIN32_WINNT=0x0501 -static-libgcc -static-libstdc++
WIN_LIBS := -lws2_32
SRCS := src/main.cpp src/config.cpp src/http_helpers.cpp src/http_request.cpp src/logging.cpp src/text_utils.cpp src/server.cpp src/server_routes.cpp src/server_transport.cpp src/session_store.cpp src/console_output.cpp src/patch_engine.cpp src/transfer_ops.cpp src/win32_error.cpp
HOST_PATCH := $(BUILD_DIR)/test_patch
HOST_TRANSFER := $(BUILD_DIR)/test_transfer
HOST_CONFIG := $(BUILD_DIR)/test_config
HOST_HTTP_REQUEST := $(BUILD_DIR)/test_http_request

all: $(TARGET)

$(TARGET): $(SRCS)
	mkdir -p $(BUILD_DIR)
	$(CXX) $(TARGET_FLAGS) -o $@ $(SRCS) $(WIN_LIBS)

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

check: test-host-patch test-host-transfer test-host-config test-host-http-request all

.PHONY: all clean test-host-patch test-host-transfer test-host-config test-host-http-request check
