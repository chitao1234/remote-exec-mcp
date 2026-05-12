#pragma once

#include <process.h>
#include <stdint.h>

#include <windows.h>
#include <winsock2.h>

typedef unsigned(__stdcall* Win32ThreadEntry)(void*);

inline HANDLE begin_win32_thread(Win32ThreadEntry entry, void* context) {
    const uintptr_t handle = _beginthreadex(NULL, 0, entry, context, 0, NULL);
    return reinterpret_cast<HANDLE>(handle);
}
