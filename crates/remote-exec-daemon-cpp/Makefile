# BSD make entry point for remote-exec-daemon-cpp host POSIX builds.
#
# GNUmakefile remains the cross-platform public entry point. Keep this file
# bmake-compatible, match the GNU host POSIX target surface, and intentionally
# leave the Windows cross-build targets to GNU make.

MAKEFILE_DIR = ${.CURDIR}/

BUILD_DIR ?= ${MAKEFILE_DIR}build
OBJ_DIR = ${BUILD_DIR}/obj

HOST_CXX ?= c++
STRESS_RUNS ?= 10
STRESS_JOBS ?= 8

COMMON_CPPFLAGS = -I${MAKEFILE_DIR}include -I${MAKEFILE_DIR}third_party
TLS ?= off
OPENSSL_ROOT ?=
OPENSSL_CPPFLAGS ?=
OPENSSL_LDLIBS ?=
.if "${TLS}" != "off" && "${TLS}" != "openssl"
.error unsupported TLS '${TLS}'; use off or openssl
.endif
.if "${TLS}" == "openssl"
COMMON_CPPFLAGS += -DREMOTE_EXEC_CPP_HAS_OPENSSL ${OPENSSL_CPPFLAGS}
.if !empty(OPENSSL_ROOT)
COMMON_CPPFLAGS += -I${OPENSSL_ROOT}/include
.endif
.if empty(OPENSSL_LDLIBS)
OPENSSL_FEATURE_LDLIBS = ${OPENSSL_ROOT:S|^|-L|:S|$|/lib|} -lssl -lcrypto
.else
OPENSSL_FEATURE_LDLIBS = ${OPENSSL_LDLIBS}
.endif
.else
OPENSSL_FEATURE_LDLIBS =
.endif
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
POSIX_CONFIG_HEADER = ${BUILD_DIR}/generated/remote_exec_cpp_config.h
POSIX_CONFIG_SCRIPT = ${MAKEFILE_DIR}scripts/write_posix_config_header.sh
POSIX_FEATURE_CPPFLAGS_SCRIPT = ${MAKEFILE_DIR}scripts/print_posix_feature_cppflags.sh
POSIX_SOCKET_LDLIBS_SCRIPT = ${MAKEFILE_DIR}scripts/print_posix_socket_ldlibs.sh
POSIX_BASE_LDLIBS = ${POSIX_PTHREAD_FLAGS}
.if !defined(POSIX_FEATURE_CPPFLAGS)
POSIX_FEATURE_CPPFLAGS != HOST_CXX="${HOST_CXX}" PROBE_CPPFLAGS="${COMMON_CPPFLAGS}" PROBE_CXXFLAGS="${PROD_CXXFLAGS} ${POSIX_PTHREAD_FLAGS}" sh "${POSIX_FEATURE_CPPFLAGS_SCRIPT}"
.endif
HOST_PROD_CPPFLAGS = -I${BUILD_DIR}/generated ${COMMON_CPPFLAGS} ${POSIX_FEATURE_CPPFLAGS}
HOST_PROD_CXXFLAGS = ${PROD_CXXFLAGS} ${POSIX_PTHREAD_FLAGS}
HOST_PROD_LDFLAGS ?= ${PROD_LDFLAGS}
HOST_TEST_CPPFLAGS = -I${BUILD_DIR}/generated ${COMMON_CPPFLAGS} ${POSIX_FEATURE_CPPFLAGS} -DREMOTE_EXEC_CPP_TESTING
HOST_TEST_CXXFLAGS = ${TEST_CXXFLAGS} ${POSIX_PTHREAD_FLAGS}
HOST_TEST_LDFLAGS ?= ${TEST_LDFLAGS}
.if !defined(POSIX_SOCKET_LDLIBS)
POSIX_SOCKET_LDLIBS != HOST_CXX="${HOST_CXX}" PROBE_CPPFLAGS="${COMMON_CPPFLAGS} ${POSIX_FEATURE_CPPFLAGS}" PROBE_CXXFLAGS="${PROD_CXXFLAGS} ${POSIX_PTHREAD_FLAGS}" PROBE_LDFLAGS="${HOST_PROD_LDFLAGS}" PROBE_LDLIBS="${POSIX_BASE_LDLIBS}" sh "${POSIX_SOCKET_LDLIBS_SCRIPT}"
.endif
HOST_PROD_LDLIBS = ${POSIX_BASE_LDLIBS} ${POSIX_SOCKET_LDLIBS} ${OPENSSL_FEATURE_LDLIBS}
HOST_TEST_LDLIBS = ${POSIX_BASE_LDLIBS} ${POSIX_SOCKET_LDLIBS} ${OPENSSL_FEATURE_LDLIBS}
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

force-posix-config:

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

${POSIX_CONFIG_HEADER}: ${POSIX_CONFIG_SCRIPT} force-posix-config
	mkdir -p ${.TARGET:H}
	HOST_CXX="${HOST_CXX}" \
	    PROBE_CPPFLAGS="${COMMON_CPPFLAGS} ${POSIX_FEATURE_CPPFLAGS}" \
	    PROBE_CXXFLAGS="${PROD_CXXFLAGS} ${POSIX_PTHREAD_FLAGS}" \
	    PROBE_LDFLAGS="${HOST_PROD_LDFLAGS}" \
	    PROBE_LDLIBS="${HOST_PROD_LDLIBS}" \
	    sh ${POSIX_CONFIG_SCRIPT} ${.TARGET}

.for test in ${HOST_POSIX_TESTS}
${HOST_${test}}: ${HOST_${test}_OBJS}
	mkdir -p ${.TARGET:H}
	${HOST_TEST_LINK}
.endfor

.for src in ${POSIX_SRCS}
${HOST_PROD_OBJ_DIR}/${src:S|.cpp|.o|}: ${src} ${POSIX_CONFIG_HEADER}
	mkdir -p ${.TARGET:H}
	${HOST_CXX} ${HOST_PROD_CPPFLAGS} ${HOST_PROD_CXXFLAGS} ${DEPFLAGS} -c -o ${.TARGET} ${src}
.endfor

.for src in ${HOST_TEST_COMPILE_SRCS}
${HOST_TEST_OBJ_DIR}/${src:S|.cpp|.o|}: ${src} ${POSIX_CONFIG_HEADER}
	mkdir -p ${.TARGET:H}
	${HOST_CXX} ${HOST_TEST_CPPFLAGS} ${HOST_TEST_CXXFLAGS} ${DEPFLAGS} -c -o ${.TARGET} ${src}
.endfor

.for dep in ${DEP_FILES}
.sinclude "${dep}"
.endfor

clean:
	rm -rf ${BUILD_DIR}

.PHONY: all all-posix check check-posix clean force-posix-config stress-posix test-server-streaming \
	${HOST_TEST_PHONY_TARGETS}
