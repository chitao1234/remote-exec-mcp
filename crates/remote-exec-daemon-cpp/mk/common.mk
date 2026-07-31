BUILD_DIR ?= $(MAKEFILE_DIR)build
OBJ_DIR := $(BUILD_DIR)/obj

COMMON_CPPFLAGS := -I$(MAKEFILE_DIR)include -I$(MAKEFILE_DIR)third_party
TLS ?= auto
include $(MAKEFILE_DIR)mk/openssl.mk
ifeq ($(filter $(TLS),auto off openssl),)
$(error unsupported TLS '$(TLS)'; use auto, off, or openssl)
endif
OPENSSL_CANDIDATE_CPPFLAGS := $(if $(OPENSSL_ROOT),-I$(OPENSSL_ROOT)/include) $(OPENSSL_CPPFLAGS)
OPENSSL_CANDIDATE_LDLIBS := $(if $(OPENSSL_LDLIBS),$(OPENSSL_LDLIBS),$(if $(OPENSSL_ROOT),-L$(OPENSSL_ROOT)/lib) -lssl -lcrypto)
DEBUG ?= 0
ifeq ($(filter 1 yes true on,$(DEBUG)),)
MODE_CXXFLAGS := -O2
else
MODE_CXXFLAGS := -O0 -g
endif
BASE_CXXFLAGS := -std=c++11 $(MODE_CXXFLAGS) -Wall -Wextra
PROD_CXXFLAGS ?= $(BASE_CXXFLAGS)
TEST_CXXFLAGS ?= $(BASE_CXXFLAGS)
HOST_TEST_CXXFLAGS := $(TEST_CXXFLAGS)
XP_TEST_CXXFLAGS ?= $(BASE_CXXFLAGS)
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
