#include <cwchar>
#include <stdexcept>
#include <string>

#include <windows.h>

#include "console_output.h"
#include "win32_error.h"

namespace {

std::string replacement_utf8() {
    return "\xEF\xBF\xBD";
}

std::string utf8_from_wide(const std::wstring& wide) {
    if (wide.empty()) {
        return "";
    }

    const int utf8_length = WideCharToMultiByte(
        CP_UTF8,
        0,
        wide.data(),
        static_cast<int>(wide.size()),
        NULL,
        0,
        NULL,
        NULL
    );
    if (utf8_length <= 0) {
        throw std::runtime_error(last_error_message("WideCharToMultiByte(CP_UTF8)"));
    }

    std::string utf8;
    utf8.resize(static_cast<std::size_t>(utf8_length));
    if (WideCharToMultiByte(
            CP_UTF8,
            0,
            wide.data(),
            static_cast<int>(wide.size()),
            &utf8[0],
            utf8_length,
            NULL,
            NULL
        ) <= 0) {
        throw std::runtime_error(last_error_message("WideCharToMultiByte(CP_UTF8)"));
    }
    return utf8;
}

std::string utf8_from_code_page(UINT code_page, const std::string& raw) {
    if (raw.empty()) {
        return "";
    }

    const int wide_length = MultiByteToWideChar(
        code_page,
        0,
        raw.data(),
        static_cast<int>(raw.size()),
        NULL,
        0
    );
    if (wide_length <= 0) {
        throw std::runtime_error(last_error_message("MultiByteToWideChar"));
    }

    std::wstring wide;
    wide.resize(static_cast<std::size_t>(wide_length));
    if (MultiByteToWideChar(
            code_page,
            0,
            raw.data(),
            static_cast<int>(raw.size()),
            &wide[0],
            wide_length
        ) <= 0) {
        throw std::runtime_error(last_error_message("MultiByteToWideChar"));
    }
    return utf8_from_wide(wide);
}

std::string decode_console_output(std::string* carry, const std::string& raw_chunk, bool flush) {
    std::string raw = *carry;
    raw += raw_chunk;
    carry->clear();

    if (raw.empty()) {
        return "";
    }

    if (!flush &&
        IsDBCSLeadByteEx(GetOEMCP(), static_cast<BYTE>(raw[raw.size() - 1])) != 0) {
        carry->push_back(raw[raw.size() - 1]);
        raw.erase(raw.size() - 1);
        if (raw.empty()) {
            return "";
        }
    }

    try {
        return utf8_from_code_page(GetOEMCP(), raw);
    } catch (const std::exception&) {
        try {
            return utf8_from_code_page(CP_ACP, raw);
        } catch (const std::exception&) {
            std::string fallback;
            for (std::size_t index = 0; index < raw.size(); ++index) {
                const unsigned char ch = static_cast<unsigned char>(raw[index]);
                if (ch == '\r' || ch == '\n' || ch == '\t' || (ch >= 0x20 && ch < 0x7F)) {
                    fallback.push_back(static_cast<char>(ch));
                } else {
                    fallback += replacement_utf8();
                }
            }
            return fallback;
        }
    }
}

std::string read_available_raw(HANDLE pipe) {
    DWORD available = 0;
    if (PeekNamedPipe(pipe, NULL, 0, NULL, &available, NULL) == 0 || available == 0) {
        return "";
    }

    std::string buffer;
    buffer.resize(available);
    DWORD read = 0;
    if (ReadFile(pipe, &buffer[0], available, &read, NULL) == 0) {
        return "";
    }
    buffer.resize(read);
    return buffer;
}

std::string read_blocking_raw(HANDLE pipe, bool* eof) {
    char buffer[4096];
    DWORD read = 0;
    if (ReadFile(pipe, buffer, sizeof(buffer), &read, NULL) == 0) {
        const DWORD error = GetLastError();
        if (error == ERROR_BROKEN_PIPE ||
            error == ERROR_NO_DATA ||
            error == ERROR_PIPE_NOT_CONNECTED) {
            *eof = true;
            return "";
        }
        throw std::runtime_error(last_error_message("ReadFile"));
    }
    if (read == 0) {
        *eof = true;
        return "";
    }
    return std::string(buffer, static_cast<std::size_t>(read));
}

}  // namespace

std::string read_available_console_output(HANDLE pipe, std::string* carry) {
    return decode_console_output(carry, read_available_raw(pipe), false);
}

std::string read_console_output(HANDLE pipe, bool block, bool* eof, std::string* carry) {
    *eof = false;
    if (block) {
        return decode_console_output(carry, read_blocking_raw(pipe, eof), false);
    }

    return decode_console_output(carry, read_available_raw(pipe), false);
}

std::string flush_console_output_carry(std::string* carry) {
    return decode_console_output(carry, "", true);
}
