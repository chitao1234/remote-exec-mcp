# BSD make entry point for remote-exec-daemon-cpp host POSIX builds.
#
# GNUmakefile remains the cross-platform public entry point. Keep this file
# bmake-compatible, match the GNU host POSIX target surface, and intentionally
# leave Windows XP cross-build targets to GNU make.

MAKEFILE_DIR = ${.CURDIR}/

BUILD_DIR ?= ${MAKEFILE_DIR}build
OBJ_DIR = ${BUILD_DIR}/obj

HOST_CXX ?= c++
STRESS_RUNS ?= 10
STRESS_JOBS ?= 8

COMMON_CPPFLAGS = -I${MAKEFILE_DIR}include -I${MAKEFILE_DIR}third_party
DEBUG ?= 0
.if "${DEBUG}" == "1" || "${DEBUG}" == "yes" || "${DEBUG}" == "true" || "${DEBUG}" == "on"
MODE_CXXFLAGS = -O0 -g
.else
MODE_CXXFLAGS = -O2
.endif
BASE_CXXFLAGS = -std=c++11 ${MODE_CXXFLAGS} -Wall -Wextra
POSIX_PTHREAD_FLAGS = -pthread
PROD_CXXFLAGS ?= ${BASE_CXXFLAGS}
TEST_CXXFLAGS ?= ${BASE_CXXFLAGS}
PROD_LDFLAGS ?=
TEST_LDFLAGS ?=
DEPFLAGS = -MMD -MP
HOST_PROD_CPPFLAGS = ${COMMON_CPPFLAGS}
HOST_PROD_CXXFLAGS = ${PROD_CXXFLAGS} ${POSIX_PTHREAD_FLAGS}
HOST_PROD_LDFLAGS ?= ${PROD_LDFLAGS}
HOST_PROD_LDLIBS = ${POSIX_PTHREAD_FLAGS}
HOST_TEST_CPPFLAGS = ${COMMON_CPPFLAGS} -DREMOTE_EXEC_CPP_TESTING
HOST_TEST_CXXFLAGS = ${TEST_CXXFLAGS} ${POSIX_PTHREAD_FLAGS}
HOST_TEST_LDFLAGS ?= ${TEST_LDFLAGS}
HOST_TEST_LDLIBS = ${POSIX_PTHREAD_FLAGS}
.if defined(REMOTE_EXEC_LOG) && !empty(REMOTE_EXEC_LOG)
TEST_LOG_LEVEL = ${REMOTE_EXEC_LOG}
.elif defined(REMOTE_EXEC_TEST_LOG) && !empty(REMOTE_EXEC_TEST_LOG)
TEST_LOG_LEVEL = ${REMOTE_EXEC_TEST_LOG}
.else
TEST_LOG_LEVEL = off
.endif
TEST_LOG_ENV = REMOTE_EXEC_LOG=${TEST_LOG_LEVEL}
HOST_TEST_LINK = ${HOST_CXX} ${HOST_TEST_CXXFLAGS} ${HOST_TEST_LDFLAGS} -o ${.TARGET} ${.ALLSRC} ${HOST_TEST_LDLIBS}

HOST_PROD_OBJ_DIR = ${OBJ_DIR}/host-prod
HOST_TEST_OBJ_DIR = ${OBJ_DIR}/host-test
POSIX_TARGET = ${BUILD_DIR}/remote-exec-daemon-cpp

SOURCE_PREFIX =
.include "${MAKEFILE_DIR}mk/sources.mk"
.include "${MAKEFILE_DIR}mk/host-tests.mk"

POSIX_OBJS = ${POSIX_SRCS:C|^|${HOST_PROD_OBJ_DIR}/|:S|.cpp|.o|:O:u}
DEP_FILES = ${POSIX_OBJS:S|.o|.d|}

.for test in ${HOST_POSIX_TESTS}
HOST_${test} = ${BUILD_DIR}/${HOST_${test}_BIN}
HOST_${test}_OBJS = ${HOST_${test}_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_TEST_SRCS += ${HOST_${test}_SRCS}
HOST_TEST_PHONY_TARGETS += ${HOST_${test}_TEST_TARGET}
DEP_FILES += ${HOST_${test}_OBJS:S|.o|.d|}
.endfor

HOST_TEST_COMPILE_SRCS = ${HOST_TEST_SRCS:O:u}

all: all-posix

all-posix: ${POSIX_TARGET}

check: check-posix

check-posix: ${HOST_TEST_PHONY_TARGETS} all-posix

stress-posix:
	@i=1; \
	while [ $$i -le ${STRESS_RUNS} ]; do \
		echo "stress-posix iteration $$i/${STRESS_RUNS}"; \
		${MAKE} check-posix -j ${STRESS_JOBS} || exit $$?; \
		i=$$((i + 1)); \
	done

test-server-streaming: ${HOST_SERVER_STREAMING_TEST_TARGET}

.for test in ${HOST_POSIX_TESTS}
${HOST_${test}_TEST_TARGET}: ${HOST_${test}}
	${TEST_LOG_ENV} ${HOST_${test}}
.endfor

${POSIX_TARGET}: ${POSIX_OBJS}
	mkdir -p ${.TARGET:H}
	${HOST_CXX} ${HOST_PROD_CXXFLAGS} ${HOST_PROD_LDFLAGS} -o ${.TARGET} ${.ALLSRC} ${HOST_PROD_LDLIBS}

.for test in ${HOST_POSIX_TESTS}
${HOST_${test}}: ${HOST_${test}_OBJS}
	mkdir -p ${.TARGET:H}
	${HOST_TEST_LINK}
.endfor

.for src in ${POSIX_SRCS}
${HOST_PROD_OBJ_DIR}/${src:S|.cpp|.o|}: ${src}
	mkdir -p ${.TARGET:H}
	${HOST_CXX} ${HOST_PROD_CPPFLAGS} ${HOST_PROD_CXXFLAGS} ${DEPFLAGS} -c -o ${.TARGET} ${src}
.endfor

.for src in ${HOST_TEST_COMPILE_SRCS}
${HOST_TEST_OBJ_DIR}/${src:S|.cpp|.o|}: ${src}
	mkdir -p ${.TARGET:H}
	${HOST_CXX} ${HOST_TEST_CPPFLAGS} ${HOST_TEST_CXXFLAGS} ${DEPFLAGS} -c -o ${.TARGET} ${src}
.endfor

.for dep in ${DEP_FILES}
.sinclude "${dep}"
.endfor

clean:
	rm -rf ${BUILD_DIR}

.PHONY: all all-posix check check-posix clean stress-posix test-server-streaming \
	${HOST_TEST_PHONY_TARGETS}
