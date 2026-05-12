# POSIX-only bmake entry point for remote-exec-daemon-cpp.
#
# GNUmakefile remains the cross-platform public entry point. Keep this file
# bmake-compatible and share only dialect-neutral source inventory.

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

POSIX_OBJS = ${POSIX_SRCS:C|^|${HOST_PROD_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_PATCH_OBJS = ${HOST_PATCH_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_TRANSFER_OBJS = ${HOST_TRANSFER_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_CONFIG_OBJS = ${HOST_CONFIG_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_BASIC_MUTEX_OBJS = ${HOST_BASIC_MUTEX_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_HTTP_REQUEST_OBJS = ${HOST_HTTP_REQUEST_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_SERVER_TRANSPORT_OBJS = ${HOST_SERVER_TRANSPORT_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_SERVER_STREAMING_OBJS = ${HOST_SERVER_STREAMING_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_SESSION_STORE_OBJS = ${HOST_SESSION_STORE_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_CONNECTION_MANAGER_OBJS = ${HOST_CONNECTION_MANAGER_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_SERVER_RUNTIME_OBJS = ${HOST_SERVER_RUNTIME_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_SERVER_ROUTES_OBJS = ${HOST_SERVER_ROUTES_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_SANDBOX_OBJS = ${HOST_SANDBOX_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}
HOST_PORT_TUNNEL_FRAME_OBJS = ${HOST_PORT_TUNNEL_FRAME_SRCS:C|^|${HOST_TEST_OBJ_DIR}/|:S|.cpp|.o|:O:u}

HOST_TEST_SRCS = ${HOST_PATCH_SRCS} ${HOST_TRANSFER_SRCS} ${HOST_CONFIG_SRCS} \
	${HOST_BASIC_MUTEX_SRCS} ${HOST_HTTP_REQUEST_SRCS} \
	${HOST_SERVER_TRANSPORT_SRCS} ${HOST_SERVER_STREAMING_SRCS} \
	${HOST_SESSION_STORE_SRCS} ${HOST_CONNECTION_MANAGER_SRCS} \
	${HOST_SERVER_RUNTIME_SRCS} ${HOST_SERVER_ROUTES_SRCS} \
	${HOST_SANDBOX_SRCS} ${HOST_PORT_TUNNEL_FRAME_SRCS}
HOST_TEST_COMPILE_SRCS = ${HOST_TEST_SRCS:O:u}

HOST_PATCH = ${BUILD_DIR}/test_patch
HOST_TRANSFER = ${BUILD_DIR}/test_transfer
HOST_CONFIG = ${BUILD_DIR}/test_config
HOST_BASIC_MUTEX = ${BUILD_DIR}/test_basic_mutex
HOST_HTTP_REQUEST = ${BUILD_DIR}/test_http_request
HOST_SERVER_TRANSPORT = ${BUILD_DIR}/test_server_transport
HOST_SERVER_STREAMING = ${BUILD_DIR}/test_server_streaming
HOST_SESSION_STORE = ${BUILD_DIR}/test_session_store
HOST_CONNECTION_MANAGER = ${BUILD_DIR}/test_connection_manager
HOST_SERVER_RUNTIME = ${BUILD_DIR}/test_server_runtime
HOST_SERVER_ROUTES = ${BUILD_DIR}/test_server_routes
HOST_SANDBOX = ${BUILD_DIR}/test_sandbox
HOST_PORT_TUNNEL_FRAME = ${BUILD_DIR}/test_port_tunnel_frame

HOST_TEST_PHONY_TARGETS = \
	test-host-basic-mutex \
	test-host-patch \
	test-host-transfer \
	test-host-config \
	test-host-http-request \
	test-host-server-transport \
	test-host-server-streaming \
	test-host-session-store \
	test-host-connection-manager \
	test-host-server-runtime \
	test-host-server-routes \
	test-host-sandbox \
	test-port-tunnel-frame

all: all-posix

all-posix: ${POSIX_TARGET}

check: check-posix

check-posix: ${HOST_TEST_PHONY_TARGETS} all-posix

test-server-streaming: test-host-server-streaming

test-host-basic-mutex: ${HOST_BASIC_MUTEX}
	${TEST_LOG_ENV} ${HOST_BASIC_MUTEX}

test-host-patch: ${HOST_PATCH}
	${TEST_LOG_ENV} ${HOST_PATCH}

test-host-transfer: ${HOST_TRANSFER}
	${TEST_LOG_ENV} ${HOST_TRANSFER}

test-host-config: ${HOST_CONFIG}
	${TEST_LOG_ENV} ${HOST_CONFIG}

test-host-http-request: ${HOST_HTTP_REQUEST}
	${TEST_LOG_ENV} ${HOST_HTTP_REQUEST}

test-host-server-transport: ${HOST_SERVER_TRANSPORT}
	${TEST_LOG_ENV} ${HOST_SERVER_TRANSPORT}

test-host-server-streaming: ${HOST_SERVER_STREAMING}
	${TEST_LOG_ENV} ${HOST_SERVER_STREAMING}

test-host-session-store: ${HOST_SESSION_STORE}
	${TEST_LOG_ENV} ${HOST_SESSION_STORE}

test-host-connection-manager: ${HOST_CONNECTION_MANAGER}
	${TEST_LOG_ENV} ${HOST_CONNECTION_MANAGER}

test-host-server-runtime: ${HOST_SERVER_RUNTIME}
	${TEST_LOG_ENV} ${HOST_SERVER_RUNTIME}

test-host-server-routes: ${HOST_SERVER_ROUTES}
	${TEST_LOG_ENV} ${HOST_SERVER_ROUTES}

test-host-sandbox: ${HOST_SANDBOX}
	${TEST_LOG_ENV} ${HOST_SANDBOX}

test-port-tunnel-frame: ${HOST_PORT_TUNNEL_FRAME}
	${TEST_LOG_ENV} ${HOST_PORT_TUNNEL_FRAME}

${POSIX_TARGET}: ${POSIX_OBJS}
	mkdir -p ${.TARGET:H}
	${HOST_CXX} ${PROD_CXXFLAGS} ${PROD_LDFLAGS} -o ${.TARGET} ${.ALLSRC} ${PTHREAD_LDLIBS}

.for test in \
	HOST_PATCH \
	HOST_TRANSFER \
	HOST_CONFIG \
	HOST_BASIC_MUTEX \
	HOST_HTTP_REQUEST \
	HOST_SERVER_TRANSPORT \
	HOST_SERVER_STREAMING \
	HOST_SESSION_STORE \
	HOST_CONNECTION_MANAGER \
	HOST_SERVER_RUNTIME \
	HOST_SERVER_ROUTES \
	HOST_SANDBOX \
	HOST_PORT_TUNNEL_FRAME
${${test}}: ${${test}_OBJS}
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
