#include <atomic>
#include <stdexcept>
#include <string>

#include <windows.h>

#include "core/logging.h"
#include "exec/console_output.h"
#include "platform/win32_error.h"
#include "platform/win32_utf8.h"

namespace {

std::string replacement_utf8() {
    return win32_utf8::replacement_utf8();
}

void log_console_decode_fallback_once(const char* fallback, const std::exception& ex, bool ansi_fallback) {
    static std::atomic<bool> logged_oem_to_ansi(false);
    static std::atomic<bool> logged_ansi_to_replacement(false);
    std::atomic<bool>* flag = ansi_fallback ? &logged_oem_to_ansi : &logged_ansi_to_replacement;
    if (flag->exchange(true)) {
        return;
    }
    log_message(LOG_WARN,
                "console_output",
                std::string("console output decode failed; falling back to ") + fallback + ": " + ex.what());
}

std::string utf8_from_wide(const std::wstring& wide) {
    return win32_utf8::utf8_from_wide(wide);
}

std::string utf8_from_code_page(UINT code_page, const std::string& raw) {
    if (raw.empty()) {
        return "";
    }

    DWORD flags = MB_ERR_INVALID_CHARS;
    int wide_length = MultiByteToWideChar(code_page, flags, raw.data(), static_cast<int>(raw.size()), nullptr, 0);
    if (wide_length <= 0 && GetLastError() == ERROR_INVALID_FLAGS) {
        flags = 0;
        wide_length = MultiByteToWideChar(code_page, flags, raw.data(), static_cast<int>(raw.size()), nullptr, 0);
    }
    if (wide_length <= 0) {
        throw std::runtime_error(last_error_message("MultiByteToWideChar"));
    }

    std::wstring wide;
    wide.resize(static_cast<std::size_t>(wide_length));
    if (MultiByteToWideChar(code_page, flags, raw.data(), static_cast<int>(raw.size()), &wide[0], wide_length) <= 0) {
        throw std::runtime_error(last_error_message("MultiByteToWideChar"));
    }
    return utf8_from_wide(wide);
}

void carry_incomplete_dbcs_suffix(UINT code_page, std::string* raw, std::string* carry) {
    for (std::size_t index = 0; index < raw->size();) {
        if (IsDBCSLeadByteEx(code_page, static_cast<BYTE>((*raw)[index])) == 0) {
            ++index;
            continue;
        }

        if (index + 1U == raw->size()) {
            carry->assign(1U, (*raw)[index]);
            raw->erase(index);
            return;
        }

        index += 2U;
    }
}

std::string decode_console_output_with_code_pages(UINT primary_code_page,
                                                  UINT fallback_code_page,
                                                  std::string* carry,
                                                  const std::string& raw_chunk,
                                                  bool flush) {
    std::string raw = *carry;
    raw += raw_chunk;
    carry->clear();

    if (raw.empty()) {
        return "";
    }

    if (!flush) {
        carry_incomplete_dbcs_suffix(primary_code_page, &raw, carry);
        if (raw.empty()) {
            return "";
        }
    }

    try {
        return utf8_from_code_page(primary_code_page, raw);
    } catch (const std::exception& ex) {
        log_console_decode_fallback_once("ANSI code page", ex, true);
        try {
            return utf8_from_code_page(fallback_code_page, raw);
        } catch (const std::exception& fallback_ex) {
            log_console_decode_fallback_once("replacement characters", fallback_ex, false);
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
    if (PeekNamedPipe(pipe, nullptr, 0, nullptr, &available, nullptr) == 0 || available == 0) {
        return "";
    }

    std::string buffer;
    buffer.resize(available);
    DWORD read = 0;
    if (ReadFile(pipe, &buffer[0], available, &read, nullptr) == 0) {
        return "";
    }
    buffer.resize(read);
    return buffer;
}

std::string read_blocking_raw(HANDLE pipe, bool* eof) {
    char buffer[4096];
    DWORD read = 0;
    if (ReadFile(pipe, buffer, sizeof(buffer), &read, nullptr) == 0) {
        const DWORD error = GetLastError();
        if (error == ERROR_BROKEN_PIPE || error == ERROR_NO_DATA || error == ERROR_PIPE_NOT_CONNECTED) {
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

} // namespace

std::string read_available_console_output(HANDLE pipe, std::string* carry) {
    return decode_console_output_with_code_pages(GetOEMCP(), CP_ACP, carry, read_available_raw(pipe), false);
}

std::string read_console_output(HANDLE pipe, bool block, bool* eof, std::string* carry) {
    *eof = false;
    if (block) {
        return decode_console_output_with_code_pages(GetOEMCP(), CP_ACP, carry, read_blocking_raw(pipe, eof), false);
    }

    return decode_console_output_with_code_pages(GetOEMCP(), CP_ACP, carry, read_available_raw(pipe), false);
}

std::string flush_console_output_carry(std::string* carry) {
    return decode_console_output_with_code_pages(GetOEMCP(), CP_ACP, carry, "", true);
}

#ifdef REMOTE_EXEC_CPP_TESTING
std::string utf8_from_windows_wide_for_test(const std::wstring& wide) {
    return utf8_from_wide(wide);
}

std::string utf8_from_windows_code_page_for_test(unsigned int code_page, const std::string& raw) {
    return utf8_from_code_page(static_cast<UINT>(code_page), raw);
}

std::string decode_console_output_for_test(unsigned int primary_code_page,
                                           unsigned int fallback_code_page,
                                           std::string* carry,
                                           const std::string& raw_chunk,
                                           bool flush) {
    return decode_console_output_with_code_pages(
        static_cast<UINT>(primary_code_page), static_cast<UINT>(fallback_code_page), carry, raw_chunk, flush);
}
#endif
