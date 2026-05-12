BUILD_DIR ?= $(MAKEFILE_DIR)build
OBJ_DIR := $(BUILD_DIR)/obj

COMMON_CPPFLAGS := -I$(MAKEFILE_DIR)include -I$(MAKEFILE_DIR)third_party
PROD_CXXFLAGS := -std=c++11 -O0 -Wall -Wextra
TEST_CXXFLAGS := -std=c++11 -O0 -Wall -Wextra
HOST_TEST_CXXFLAGS := $(TEST_CXXFLAGS)
XP_TEST_CXXFLAGS := -std=c++11 -O0 -Wall -Wextra
DEPFLAGS := -MMD -MP
DEP_FILES :=
TEST_LOG_LEVEL := $(if $(REMOTE_EXEC_LOG),$(REMOTE_EXEC_LOG),$(if $(REMOTE_EXEC_TEST_LOG),$(REMOTE_EXEC_TEST_LOG),off))

SOURCE_PREFIX := $(MAKEFILE_DIR)
include $(MAKEFILE_DIR)mk/sources.mk

cpp_objs = $(patsubst $(MAKEFILE_DIR)%.cpp,$(1)/%.o,$(2))

define run_test
$1: $2
	REMOTE_EXEC_LOG=$(TEST_LOG_LEVEL) $2
endef
