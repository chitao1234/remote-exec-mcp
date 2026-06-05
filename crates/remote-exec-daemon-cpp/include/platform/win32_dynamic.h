#pragma once

#ifdef _WIN32

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif

#ifndef NOMINMAX
#define NOMINMAX
#endif

#include <windows.h>

namespace remote_exec_win32 {

template <typename Fn> Fn proc_address_as(FARPROC proc) {
    union ProcAddressCast {
        FARPROC proc;
        Fn fn;
    } cast = {proc};
    return cast.fn;
}

} // namespace remote_exec_win32

#endif
