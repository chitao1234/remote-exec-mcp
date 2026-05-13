# POSIX-only bmake entry point for remote-exec-daemon-cpp.
#
# GNUmakefile remains the cross-platform public entry point. Keep this file
# bmake-compatible and share only dialect-neutral inventory files.

BUILD_DIR ?= build
OBJ_DIR = ${BUILD_DIR}/obj

HOST_CXX ?= g++

COMMON_CPPFLAGS = -Iinclude -Ithird_party
PROD_CXXFLAGS = -std=c++11 -O0 -Wall -Wextra -pthread
TEST_CXXFLAGS = -std=c++11 -O0 -Wall -Wextra -pthread
PROD_LDFLAGS ?=
TEST_LDFLAGS ?=
PTHREAD_LDLIBS = -pthread
.if defined(REMOTE_EXEC_LOG) && !empty(REMOTE_EXEC_LOG)
TEST_LOG_LEVEL = ${REMOTE_EXEC_LOG}
.elif defined(REMOTE_EXEC_TEST_LOG) && !empty(REMOTE_EXEC_TEST_LOG)
TEST_LOG_LEVEL = ${REMOTE_EXEC_TEST_LOG}
.else
TEST_LOG_LEVEL = off
.endif
TEST_LOG_ENV = REMOTE_EXEC_LOG=${TEST_LOG_LEVEL}
HOST_TEST_LINK = ${HOST_CXX} ${TEST_CXXFLAGS} ${TEST_LDFLAGS} -o ${.TARGET} ${.ALLSRC} ${PTHREAD_LDLIBS}

HOST_PROD_OBJ_DIR = ${OBJ_DIR}/host-prod
HOST_TEST_OBJ_DIR = ${OBJ_DIR}/host-test
POSIX_TARGET = ${BUILD_DIR}/remote-exec-daemon-cpp

SOURCE_PREFIX =
.include "mk/sources.mk"
.include "mk/host-tests.mk"

POSIX_OBJS = ${POSIX_SRCS:C|^|${HOST_PROD_OBJ_DIR}/|:S|.cpp|.o|:O:u}

.for test in ${HOST_POSIX_TESTS}
HOST_${test} = ${BUILD_DIR}/${HOST_${test}_BIN}
HOST_${test}_OBJS = ${HOST_${test}_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_TEST_SRCS += ${HOST_${test}_SRCS}
HOST_TEST_PHONY_TARGETS += ${HOST_${test}_TEST_TARGET}
.endfor

HOST_TEST_COMPILE_SRCS = ${HOST_TEST_SRCS:O:u}

all: all-posix

all-posix: ${POSIX_TARGET}

check: check-posix

check-posix: ${HOST_TEST_PHONY_TARGETS} all-posix

test-server-streaming: ${HOST_SERVER_STREAMING_TEST_TARGET}

.for test in ${HOST_POSIX_TESTS}
${HOST_${test}_TEST_TARGET}: ${HOST_${test}}
	${TEST_LOG_ENV} ${HOST_${test}}
.endfor

${POSIX_TARGET}: ${POSIX_OBJS}
	mkdir -p ${.TARGET:H}
	${HOST_CXX} ${PROD_CXXFLAGS} ${PROD_LDFLAGS} -o ${.TARGET} ${.ALLSRC} ${PTHREAD_LDLIBS}

.for test in ${HOST_POSIX_TESTS}
${HOST_${test}}: ${HOST_${test}_OBJS}
	mkdir -p ${.TARGET:H}
	${HOST_TEST_LINK}
.endfor

.for src in ${POSIX_SRCS}
${HOST_PROD_OBJ_DIR}/${src:S|.cpp|.o|}: ${src}
	mkdir -p ${.TARGET:H}
	${HOST_CXX} ${COMMON_CPPFLAGS} ${PROD_CXXFLAGS} -c -o ${.TARGET} ${.ALLSRC}
.endfor

.for src in ${HOST_TEST_COMPILE_SRCS}
${HOST_TEST_OBJ_DIR}/${src:S|.cpp|.o|}: ${src}
	mkdir -p ${.TARGET:H}
	${HOST_CXX} ${COMMON_CPPFLAGS} -DREMOTE_EXEC_CPP_TESTING ${TEST_CXXFLAGS} -c -o ${.TARGET} ${.ALLSRC}
.endfor

clean:
	rm -rf ${BUILD_DIR}

.PHONY: all all-posix check check-posix clean test-server-streaming \
	${HOST_TEST_PHONY_TARGETS}
