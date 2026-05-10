MAKEFILE_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
.DEFAULT_GOAL := all

include $(MAKEFILE_DIR)mk/common.mk
include $(MAKEFILE_DIR)mk/posix.mk
include $(MAKEFILE_DIR)mk/windows-xp.mk

# Default commands are host-native only. Windows XP is an explicit cross-build.
all: all-posix

check: check-posix

clean:
	rm -rf $(BUILD_DIR)

-include $(DEP_FILES)

.PHONY: all check clean
