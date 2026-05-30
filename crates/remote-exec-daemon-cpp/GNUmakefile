MAKEFILE_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
.DEFAULT_GOAL := all

include $(MAKEFILE_DIR)mk/common.mk
include $(MAKEFILE_DIR)mk/posix.mk
include $(MAKEFILE_DIR)mk/windows-xp.mk
include $(MAKEFILE_DIR)mk/windows-winsock1.mk
ifeq ($(OS),Windows_NT)
include $(MAKEFILE_DIR)mk/windows-native.mk
endif

# Default commands are host-native only. Windows cross-build targets are explicit.
all: all-posix

check: check-posix

clean:
	rm -rf $(BUILD_DIR)

-include $(DEP_FILES)

.PHONY: all check clean
