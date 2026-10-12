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
    // GetProcAddress supplies an untyped function address. The intermediate
    // void() pointer avoids GCC's incompatible-signature warning; it is never
    // called, and the final pointer must match the exported function's signature.
    return reinterpret_cast<Fn>(reinterpret_cast<void (*)()>(proc));
}

} // namespace remote_exec_win32

#endif
