#include <cstdlib>
#include <cwchar>
#include <sstream>
#include <stdexcept>
#include <vector>

#include "logging.h"
#include "session_store.h"

static std::string make_chunk_id() {
    std::ostringstream out;
    out << "xp-" << GetTickCount() << '-' << std::rand();
    return out.str();
}

static std::string last_error_message(const char* prefix) {
    std::ostringstream out;
    out << prefix << " failed with error " << GetLastError();
    return out.str();
}

static std::string replacement_utf8() {
    return "\xEF\xBF\xBD";
}

static std::string utf8_from_wide(const std::wstring& wide) {
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

static std::string utf8_from_code_page(UINT code_page, const std::string& raw) {
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

static std::string decode_console_output(
    std::string* carry,
    const std::string& raw_chunk,
    bool flush
) {
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

static void close_live_session(const std::shared_ptr<LiveSession>& session) {
    if (session->stdin_write != NULL) {
        CloseHandle(session->stdin_write);
        session->stdin_write = NULL;
    }
    if (session->stdout_read != NULL) {
        CloseHandle(session->stdout_read);
        session->stdout_read = NULL;
    }
    if (session->process_handle != NULL) {
        CloseHandle(session->process_handle);
        session->process_handle = NULL;
    }
}

static std::string trim_output(const std::string& output, unsigned long max_output_chars) {
    if (max_output_chars == 0 || output.size() <= max_output_chars) {
        return output;
    }
    return output.substr(output.size() - max_output_chars);
}

static double wall_time_seconds(DWORD started_at_ms) {
    return static_cast<double>(GetTickCount() - started_at_ms) / 1000.0;
}

static std::string read_available_raw(HANDLE pipe) {
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

static std::string read_available(const std::shared_ptr<LiveSession>& session) {
    return decode_console_output(
        &session->output_carry,
        read_available_raw(session->stdout_read),
        false
    );
}

static std::string flush_output_carry(const std::shared_ptr<LiveSession>& session) {
    return decode_console_output(&session->output_carry, "", true);
}

static Json build_response(
    const char* daemon_session_id,
    bool running,
    DWORD started_at_ms,
    bool has_exit_code,
    DWORD exit_code,
    const std::string& output,
    unsigned long max_output_chars
) {
    const std::string trimmed = trim_output(output, max_output_chars);
    unsigned long original_token_count = 0;
    {
        std::istringstream tokens(output);
        std::string token;
        while (tokens >> token) {
            ++original_token_count;
        }
    }
    return Json{
        {"daemon_session_id", daemon_session_id != NULL ? Json(daemon_session_id) : Json(nullptr)},
        {"running", running},
        {"chunk_id", make_chunk_id()},
        {"wall_time_seconds", wall_time_seconds(started_at_ms)},
        {"exit_code", has_exit_code ? Json(static_cast<int>(exit_code)) : Json(nullptr)},
        {"original_token_count", original_token_count},
        {"output", trimmed},
        {"warnings", Json::array()}
    };
}

static unsigned long clamp_timeout(
    unsigned long requested_ms,
    unsigned long fallback_ms,
    unsigned long minimum_ms,
    unsigned long maximum_ms
) {
    unsigned long value = requested_ms == 0 ? fallback_ms : requested_ms;
    if (value < minimum_ms) {
        value = minimum_ms;
    }
    if (value > maximum_ms) {
        value = maximum_ms;
    }
    return value;
}

SessionStore::SessionStore() {
    std::srand(static_cast<unsigned int>(GetTickCount()));
}

SessionStore::~SessionStore() {
    for (std::map<std::string, std::shared_ptr<LiveSession> >::iterator it = sessions_.begin();
         it != sessions_.end();
         ++it) {
        const std::shared_ptr<LiveSession>& session = it->second;
        if (session->process_handle != NULL) {
            TerminateProcess(session->process_handle, 1);
        }
        close_live_session(session);
    }
}

Json SessionStore::start_command(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    unsigned long yield_time_ms,
    unsigned long max_output_chars
) {
    {
        std::ostringstream message;
        message << "start_command cmd_preview=`" << preview_text(command, 120)
                << "` workdir=`" << workdir << "` shell=`"
                << (shell.empty() ? "cmd.exe" : shell) << '`';
        log_message(LOG_INFO, "session_store", message.str());
    }
    SECURITY_ATTRIBUTES sa;
    sa.nLength = sizeof(sa);
    sa.lpSecurityDescriptor = NULL;
    sa.bInheritHandle = TRUE;

    HANDLE stdout_read = NULL;
    HANDLE stdout_write = NULL;
    HANDLE stdin_read = NULL;
    HANDLE stdin_write = NULL;

    if (CreatePipe(&stdout_read, &stdout_write, &sa, 0) == 0) {
        throw std::runtime_error(last_error_message("CreatePipe(stdout)"));
    }
    if (CreatePipe(&stdin_read, &stdin_write, &sa, 0) == 0) {
        CloseHandle(stdout_read);
        CloseHandle(stdout_write);
        throw std::runtime_error(last_error_message("CreatePipe(stdin)"));
    }
    SetHandleInformation(stdout_read, HANDLE_FLAG_INHERIT, 0);
    SetHandleInformation(stdin_write, HANDLE_FLAG_INHERIT, 0);

    STARTUPINFOA startup_info;
    ZeroMemory(&startup_info, sizeof(startup_info));
    startup_info.cb = sizeof(startup_info);
    startup_info.dwFlags = STARTF_USESTDHANDLES;
    startup_info.hStdInput = stdin_read;
    startup_info.hStdOutput = stdout_write;
    startup_info.hStdError = stdout_write;

    PROCESS_INFORMATION process_info;
    ZeroMemory(&process_info, sizeof(process_info));

    const std::string resolved_shell = shell.empty() ? "cmd.exe" : shell;
    std::string command_line = resolved_shell + " /C " + command;
    std::vector<char> mutable_command_line(command_line.begin(), command_line.end());
    mutable_command_line.push_back('\0');

    const BOOL created = CreateProcessA(
        NULL,
        &mutable_command_line[0],
        NULL,
        NULL,
        TRUE,
        0,
        NULL,
        workdir.empty() ? NULL : workdir.c_str(),
        &startup_info,
        &process_info
    );
    CloseHandle(stdin_read);
    CloseHandle(stdout_write);

    if (created == 0) {
        CloseHandle(stdin_write);
        CloseHandle(stdout_read);
        throw std::runtime_error(last_error_message("CreateProcessA"));
    }

    CloseHandle(process_info.hThread);

    std::shared_ptr<LiveSession> session(new LiveSession());
    session->id = make_chunk_id();
    session->process_handle = process_info.hProcess;
    session->stdin_write = stdin_write;
    session->stdout_read = stdout_read;
    session->started_at_ms = GetTickCount();

    const unsigned long timeout_ms = clamp_timeout(yield_time_ms, 10000, 250, 30000);
    const DWORD poll_start = GetTickCount();
    std::string output;

    while (GetTickCount() - poll_start < timeout_ms) {
        output += read_available(session);

        if (WaitForSingleObject(session->process_handle, 0) == WAIT_OBJECT_0) {
            output += read_available(session);
            output += flush_output_carry(session);
            DWORD exit_code = 0;
            GetExitCodeProcess(session->process_handle, &exit_code);
            Json response = build_response(
                NULL,
                false,
                session->started_at_ms,
                true,
                exit_code,
                output,
                max_output_chars
            );
            {
                std::ostringstream message;
                message << "command completed before session handoff exit_code=" << exit_code
                        << " output_chars=" << output.size();
                log_message(LOG_INFO, "session_store", message.str());
            }
            close_live_session(session);
            return response;
        }

        Sleep(25);
    }

    sessions_[session->id] = session;
    {
        std::ostringstream message;
        message << "stored live session daemon_session_id=`" << session->id
                << "` open_sessions=" << sessions_.size();
        log_message(LOG_INFO, "session_store", message.str());
    }
    return build_response(
        session->id.c_str(),
        true,
        session->started_at_ms,
        false,
        0,
        output,
        max_output_chars
    );
}

Json SessionStore::write_stdin(
    const std::string& daemon_session_id,
    const std::string& chars,
    unsigned long yield_time_ms,
    unsigned long max_output_chars
) {
    std::map<std::string, std::shared_ptr<LiveSession> >::iterator it = sessions_.find(daemon_session_id);
    if (it == sessions_.end()) {
        log_message(
            LOG_WARN,
            "session_store",
            "unknown daemon session `" + daemon_session_id + "`"
        );
        throw std::runtime_error("unknown_session");
    }

    {
        std::ostringstream message;
        message << "write_stdin daemon_session_id=`" << daemon_session_id
                << "` chars_len=" << chars.size();
        log_message(LOG_INFO, "session_store", message.str());
    }

    const std::shared_ptr<LiveSession>& session = it->second;
    if (!chars.empty()) {
        DWORD written = 0;
        if (WriteFile(
                session->stdin_write,
                chars.data(),
                static_cast<DWORD>(chars.size()),
                &written,
                NULL
            ) == 0) {
            throw std::runtime_error(last_error_message("WriteFile"));
        }
    }

    const unsigned long timeout_ms = clamp_timeout(yield_time_ms, 250, 250, 30000);
    const DWORD poll_start = GetTickCount();
    std::string output;

    while (GetTickCount() - poll_start < timeout_ms) {
        output += read_available(session);

        if (WaitForSingleObject(session->process_handle, 0) == WAIT_OBJECT_0) {
            output += read_available(session);
            output += flush_output_carry(session);
            DWORD exit_code = 0;
            GetExitCodeProcess(session->process_handle, &exit_code);
            Json response = build_response(
                NULL,
                false,
                session->started_at_ms,
                true,
                exit_code,
                output,
                max_output_chars
            );
            close_live_session(session);
            sessions_.erase(it);
            {
                std::ostringstream message;
                message << "session completed daemon_session_id=`" << daemon_session_id
                        << "` exit_code=" << exit_code
                        << " open_sessions=" << sessions_.size();
                log_message(LOG_INFO, "session_store", message.str());
            }
            return response;
        }

        Sleep(25);
    }

    {
        std::ostringstream message;
        message << "session still running daemon_session_id=`" << session->id << '`';
        log_message(LOG_INFO, "session_store", message.str());
    }
    return build_response(
        session->id.c_str(),
        true,
        session->started_at_ms,
        false,
        0,
        output,
        max_output_chars
    );
}
