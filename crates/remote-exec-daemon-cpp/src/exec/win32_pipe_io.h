#pragma once

#ifdef _WIN32

#include <string>

#include <windows.h>

#include "platform/win32_error.h"

// Shared Win32 pipe helpers used by console output decoding and process
// session stdout/stderr reads.

// Returns true when a pipe read/write failed because the peer closed the pipe
// (or the pipe was never connected).
inline bool is_win32_pipe_closed_error(DWORD error) {
    return error == ERROR_BROKEN_PIPE || error == ERROR_NO_DATA
           || error == ERROR_PIPE_NOT_CONNECTED;
}

// Reads whatever bytes are currently available on `pipe` without blocking.
// Returns "" when the pipe has no data or the peer closed it (setting *eof
// when the close was observed). When throw_on_error is true, unexpected
// errors raise instead of being treated as an empty read.
inline std::string read_pipe_available_raw(HANDLE pipe, bool* eof, bool throw_on_error) {
    DWORD available = 0;
    if (PeekNamedPipe(pipe, nullptr, 0, nullptr, &available, nullptr) == 0) {
        const DWORD error = GetLastError();
        if (is_win32_pipe_closed_error(error)) {
            if (eof != nullptr) {
                *eof = true;
            }
            return "";
        }
        if (throw_on_error) {
            throw std::runtime_error(last_error_message("PeekNamedPipe"));
        }
        return "";
    }

    if (available == 0) {
        return "";
    }

    std::string buffer;
    buffer.resize(static_cast<std::size_t>(available));
    DWORD read = 0;
    if (ReadFile(pipe, &buffer[0], available, &read, nullptr) == 0) {
        const DWORD error = GetLastError();
        if (is_win32_pipe_closed_error(error)) {
            if (eof != nullptr) {
                *eof = true;
            }
            return "";
        }
        if (throw_on_error) {
            throw std::runtime_error(last_error_message("ReadFile"));
        }
        return "";
    }

    if (read == 0U) {
        if (eof != nullptr) {
            *eof = true;
        }
        return "";
    }

    buffer.resize(static_cast<std::size_t>(read));
    return buffer;
}

#endif
