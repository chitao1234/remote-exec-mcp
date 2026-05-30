MAKEFILE_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
.DEFAULT_GOAL := all

include $(MAKEFILE_DIR)mk/common.mk
include $(MAKEFILE_DIR)mk/posix.mk
include $(MAKEFILE_DIR)mk/windows-gnu.mk

# Default commands are host-native only. Windows cross-build targets are explicit.
all: all-posix

check: check-posix

clean:
	rm -rf $(BUILD_DIR)

-include $(DEP_FILES)

.PHONY: all check clean
