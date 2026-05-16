#include <sstream>
#include <string>

#include <windows.h>

#include "path_utils.h"
#include "win32_error.h"

namespace {

std::wstring trim_trailing_whitespace(const std::wstring& value) {
    std::wstring trimmed = value;
    while (!trimmed.empty()) {
        const wchar_t last = trimmed[trimmed.size() - 1];
        if (last == L' ' || last == L'\t' || last == L'\r' || last == L'\n') {
            trimmed.erase(trimmed.size() - 1);
            continue;
        }
        break;
    }
    return trimmed;
}

std::wstring system_message_from_code(unsigned long error) {
    wchar_t* buffer = nullptr;
    const DWORD flags =
        FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS;
    const DWORD written = FormatMessageW(
        flags,
        NULL,
        static_cast<DWORD>(error),
        0,
        reinterpret_cast<wchar_t*>(&buffer),
        0,
        NULL);
    if (written == 0 || buffer == NULL) {
        return std::wstring();
    }

    std::wstring message(buffer, static_cast<std::size_t>(written));
    LocalFree(buffer);
    return trim_trailing_whitespace(message);
}

} // namespace

std::string error_message_from_code(const char* prefix, unsigned long error) {
    std::ostringstream out;
    out << prefix << " failed";

    const std::wstring wide_message = system_message_from_code(error);
    if (!wide_message.empty()) {
        try {
            out << ": " << path_utils::utf8_from_wide(wide_message) << " (error " << error << ")";
            return out.str();
        } catch (const std::exception&) {
        }
    }

    out << " with error " << error;
    return out.str();
}

std::string last_error_message(const char* prefix) {
    return error_message_from_code(prefix, GetLastError());
}
