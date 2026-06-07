#pragma once

#ifdef _WIN32

#include <cstring>
#include <stdexcept>
#include <string>

#include <windows.h>

#include "platform/win32_utf8.h"

namespace remote_exec_win32 {

#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
typedef char NativeChar;
typedef std::string NativeString;
typedef STARTUPINFOA NativeStartupInfo;
typedef WIN32_FIND_DATAA NativeFindData;
#else
typedef wchar_t NativeChar;
typedef std::wstring NativeString;
typedef STARTUPINFOW NativeStartupInfo;
typedef WIN32_FIND_DATAW NativeFindData;
#endif

inline const char* native_api_suffix() {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return "A";
#else
    return "W";
#endif
}

inline std::string native_api_name(const char* base_name) {
    return std::string(base_name == nullptr ? "" : base_name) + native_api_suffix();
}

inline NativeChar native_char(char ch) {
    return static_cast<NativeChar>(static_cast<unsigned char>(ch));
}

inline NativeString native_from_ascii(const char* value) {
    NativeString output;
    if (value == nullptr) {
        return output;
    }
    while (*value != '\0') {
        output.push_back(native_char(*value));
        ++value;
    }
    return output;
}

#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
inline std::string ansi_from_wide(const std::wstring& wide, const char* context) {
    if (wide.empty()) {
        return std::string();
    }

    BOOL used_default_char = FALSE;
    int length = WideCharToMultiByte(
        CP_ACP,
        0,
        wide.data(),
        static_cast<int>(wide.size()),
        nullptr,
        0,
        nullptr,
        &used_default_char
    );
    if (length <= 0) {
        throw std::runtime_error(
            std::string("unable to encode UTF-8 as Windows ANSI for ")
            + (context == nullptr ? "Win32 call" : context)
        );
    }

    std::string ansi;
    ansi.resize(static_cast<std::size_t>(length));
    used_default_char = FALSE;
    length = WideCharToMultiByte(
        CP_ACP,
        0,
        wide.data(),
        static_cast<int>(wide.size()),
        &ansi[0],
        static_cast<int>(ansi.size()),
        nullptr,
        &used_default_char
    );
    if (length <= 0 || static_cast<std::size_t>(length) != ansi.size()) {
        throw std::runtime_error(
            std::string("unable to encode UTF-8 as Windows ANSI for ")
            + (context == nullptr ? "Win32 call" : context)
        );
    }
    if (used_default_char != FALSE) {
        throw std::runtime_error(
            std::string("UTF-8 text is not representable in the Windows ANSI code page for ")
            + (context == nullptr ? "Win32 call" : context)
        );
    }
    return ansi;
}

inline std::wstring wide_from_ansi(const std::string& ansi, const char* context) {
    if (ansi.empty()) {
        return std::wstring();
    }

    int length =
        MultiByteToWideChar(CP_ACP, 0, ansi.data(), static_cast<int>(ansi.size()), nullptr, 0);
    if (length <= 0) {
        throw std::runtime_error(
            std::string("unable to decode Windows ANSI text as Unicode for ")
            + (context == nullptr ? "Win32 call" : context)
        );
    }

    std::wstring wide;
    wide.resize(static_cast<std::size_t>(length));
    length = MultiByteToWideChar(
        CP_ACP,
        0,
        ansi.data(),
        static_cast<int>(ansi.size()),
        &wide[0],
        static_cast<int>(wide.size())
    );
    if (length <= 0 || static_cast<std::size_t>(length) != wide.size()) {
        throw std::runtime_error(
            std::string("unable to decode Windows ANSI text as Unicode for ")
            + (context == nullptr ? "Win32 call" : context)
        );
    }
    return wide;
}
#endif

inline NativeString native_from_utf8(const std::string& value, const char* context) {
    try {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
        return ansi_from_wide(win32_utf8::wide_from_utf8(value), context);
#else
        (void)context;
        return win32_utf8::wide_from_utf8(value);
#endif
    } catch (const std::exception& ex) {
        throw std::runtime_error(
            std::string("unable to convert UTF-8 text for ")
            + (context == nullptr ? "Win32 call" : context) + ": " + ex.what()
        );
    }
}

inline std::string utf8_from_native(const NativeString& value, const char* context) {
    try {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
        return win32_utf8::utf8_from_wide(wide_from_ansi(value, context));
#else
        (void)context;
        return win32_utf8::utf8_from_wide(value);
#endif
    } catch (const std::exception& ex) {
        throw std::runtime_error(
            std::string("unable to convert Win32 text to UTF-8 for ")
            + (context == nullptr ? "Win32 call" : context) + ": " + ex.what()
        );
    }
}

inline std::string utf8_from_native(const NativeChar* value, const char* context) {
    if (value == nullptr) {
        return std::string();
    }
    return utf8_from_native(NativeString(value), context);
}

inline HANDLE find_first_file_native(const NativeChar* path, NativeFindData* data) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return FindFirstFileA(path, data);
#else
    return FindFirstFileW(path, data);
#endif
}

inline BOOL find_next_file_native(HANDLE handle, NativeFindData* data) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return FindNextFileA(handle, data);
#else
    return FindNextFileW(handle, data);
#endif
}

inline DWORD get_file_attributes_native(const NativeChar* path) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return GetFileAttributesA(path);
#else
    return GetFileAttributesW(path);
#endif
}

inline BOOL create_directory_native(const NativeChar* path) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return CreateDirectoryA(path, nullptr);
#else
    return CreateDirectoryW(path, nullptr);
#endif
}

inline BOOL delete_file_native(const NativeChar* path) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return DeleteFileA(path);
#else
    return DeleteFileW(path);
#endif
}

inline BOOL remove_directory_native(const NativeChar* path) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return RemoveDirectoryA(path);
#else
    return RemoveDirectoryW(path);
#endif
}

inline DWORD get_full_path_name_native(
    const NativeChar* path,
    DWORD buffer_length,
    NativeChar* buffer,
    NativeChar** file_part
) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return GetFullPathNameA(path, buffer_length, buffer, file_part);
#else
    return GetFullPathNameW(path, buffer_length, buffer, file_part);
#endif
}

inline DWORD get_short_path_name_native(
    const NativeChar* path,
    NativeChar* buffer,
    DWORD buffer_length
) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return GetShortPathNameA(path, buffer, buffer_length);
#else
    return GetShortPathNameW(path, buffer, buffer_length);
#endif
}

inline DWORD get_module_file_name_native(HMODULE module, NativeChar* buffer, DWORD buffer_length) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return GetModuleFileNameA(module, buffer, buffer_length);
#else
    return GetModuleFileNameW(module, buffer, buffer_length);
#endif
}

inline HANDLE create_file_native(
    const NativeChar* name,
    DWORD desired_access,
    DWORD share_mode,
    LPSECURITY_ATTRIBUTES security_attributes,
    DWORD creation_disposition,
    DWORD flags_and_attributes,
    HANDLE template_file
) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return CreateFileA(
        name,
        desired_access,
        share_mode,
        security_attributes,
        creation_disposition,
        flags_and_attributes,
        template_file
    );
#else
    return CreateFileW(
        name,
        desired_access,
        share_mode,
        security_attributes,
        creation_disposition,
        flags_and_attributes,
        template_file
    );
#endif
}

inline BOOL create_process_native(
    const NativeChar* application_name,
    NativeChar* command_line,
    LPSECURITY_ATTRIBUTES process_attributes,
    LPSECURITY_ATTRIBUTES thread_attributes,
    BOOL inherit_handles,
    DWORD creation_flags,
    LPVOID environment,
    const NativeChar* current_directory,
    NativeStartupInfo* startup_info,
    LPPROCESS_INFORMATION process_information
) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return CreateProcessA(
        application_name,
        command_line,
        process_attributes,
        thread_attributes,
        inherit_handles,
        creation_flags,
        environment,
        current_directory,
        startup_info,
        process_information
    );
#else
    return CreateProcessW(
        application_name,
        command_line,
        process_attributes,
        thread_attributes,
        inherit_handles,
        creation_flags,
        environment,
        current_directory,
        startup_info,
        process_information
    );
#endif
}

} // namespace remote_exec_win32

#endif
