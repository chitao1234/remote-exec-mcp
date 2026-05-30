#pragma once

#ifdef _WIN32

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif

#ifndef NOMINMAX
#define NOMINMAX
#endif

#ifdef REMOTE_EXEC_CPP_WINSOCK1
#include <winsock.h>

typedef int socklen_t;

struct sockaddr_storage {
    short ss_family;
    long __ss_align;
    char __ss_pad[120];
};

#ifndef SD_RECEIVE
#define SD_RECEIVE 0
#endif
#ifndef SD_SEND
#define SD_SEND 1
#endif
#ifndef SD_BOTH
#define SD_BOTH 2
#endif

#else
#include <winsock2.h>
#include <ws2tcpip.h>
#endif

#endif
