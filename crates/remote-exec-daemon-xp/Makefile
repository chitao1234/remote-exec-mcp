CXX = i686-w64-mingw32-g++
HOST_CXX ?= g++
BUILD_DIR := build
TARGET := $(BUILD_DIR)/remote-exec-daemon-xp.exe
COMMON_FLAGS := -std=gnu++17 -O0 -Wall -Wextra -Iinclude -Ithird_party
TARGET_FLAGS := $(COMMON_FLAGS) -DWINVER=0x0501 -D_WIN32_WINNT=0x0501 -static-libgcc -static-libstdc++
WIN_LIBS := -lws2_32
SRCS := src/main.cpp src/config.cpp src/http_helpers.cpp src/logging.cpp src/server.cpp src/session_store.cpp src/patch_engine.cpp src/transfer_ops.cpp
HOST_PATCH := $(BUILD_DIR)/test_patch
HOST_TRANSFER := $(BUILD_DIR)/test_transfer

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

check: test-host-patch test-host-transfer all

.PHONY: all clean test-host-patch test-host-transfer check
