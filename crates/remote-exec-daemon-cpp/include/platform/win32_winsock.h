#pragma once

#ifdef _WIN32

#include "platform/win32_socket_compat.h"

namespace remote_exec_win32 {

inline bool winsock_major_version_is(WORD version, unsigned int major) {
    return static_cast<unsigned int>(LOBYTE(version)) == major;
}

inline int start_winsock(WSADATA* wsa_data) {
#ifdef REMOTE_EXEC_CPP_WINSOCK1
    const int status = WSAStartup(MAKEWORD(1, 1), wsa_data);
    if (status != 0) {
        return status;
    }
    if (winsock_major_version_is(wsa_data->wVersion, 1) && HIBYTE(wsa_data->wVersion) >= 1) {
        return 0;
    }
    WSACleanup();
    return WSAVERNOTSUPPORTED;
#else
    const WORD requested_versions[] = {MAKEWORD(2, 2), MAKEWORD(2, 1), MAKEWORD(2, 0)};
    int last_status = WSAVERNOTSUPPORTED;
    for (int i = 0; i < static_cast<int>(sizeof(requested_versions) / sizeof(requested_versions[0])); ++i) {
        const int status = WSAStartup(requested_versions[i], wsa_data);
        if (status != 0) {
            last_status = status;
            continue;
        }
        if (winsock_major_version_is(wsa_data->wVersion, 2)) {
            return 0;
        }
        WSACleanup();
        last_status = WSAVERNOTSUPPORTED;
    }
    return last_status;
#endif
}

} // namespace remote_exec_win32

#endif
