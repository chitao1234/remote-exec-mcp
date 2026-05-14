#ifdef _WIN32

#include <sstream>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include <windows.h>
#include <winsock2.h>

#include "console_output.h"
#include "platform.h"
#include "process_session.h"
#include "win32_error.h"
#include "win32_scoped.h"

namespace {

std::wstring wide_from_utf8(const std::string& value) {
    if (value.empty()) {
        return std::wstring();
    }

    const int wide_length =
        MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), NULL, 0);
    if (wide_length <= 0) {
        throw std::runtime_error(last_error_message("MultiByteToWideChar(CP_UTF8)"));
    }

    std::wstring wide(static_cast<std::size_t>(wide_length), L'\0');
    if (MultiByteToWideChar(
            CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), &wide[0], wide_length) <=
        0) {
        throw std::runtime_error(last_error_message("MultiByteToWideChar(CP_UTF8)"));
    }
    return wide;
}

std::string windows_quote_arg(const std::string& arg) {
    if (arg.empty()) {
        return "\"\"";
    }

    bool needs_quotes = false;
    for (std::size_t i = 0; i < arg.size(); ++i) {
        if (arg[i] == ' ' || arg[i] == '\t' || arg[i] == '"') {
            needs_quotes = true;
            break;
        }
    }
    if (!needs_quotes) {
        return arg;
    }

    std::string quoted = "\"";
    std::size_t backslashes = 0;
    for (std::size_t i = 0; i < arg.size(); ++i) {
        const char ch = arg[i];
        if (ch == '\\') {
            ++backslashes;
            continue;
        }
        if (ch == '"') {
            quoted.append(backslashes * 2U + 1U, '\\');
            quoted.push_back('"');
            backslashes = 0;
            continue;
        }
        quoted.append(backslashes, '\\');
        backslashes = 0;
        quoted.push_back(ch);
    }
    quoted.append(backslashes * 2U, '\\');
    quoted.push_back('"');
    return quoted;
}

std::string command_line_from_argv(const std::vector<std::string>& argv) {
    std::ostringstream out;
    for (std::size_t i = 0; i < argv.size(); ++i) {
        if (i != 0) {
            out << ' ';
        }
        out << windows_quote_arg(argv[i]);
    }
    return out.str();
}

struct PipePair {
    UniqueHandle read_end;
    UniqueHandle write_end;
};

PipePair create_pipe_pair(const char* label) {
    SECURITY_ATTRIBUTES sa;
    sa.nLength = sizeof(sa);
    sa.lpSecurityDescriptor = NULL;
    sa.bInheritHandle = TRUE;

    HANDLE read_end = NULL;
    HANDLE write_end = NULL;
    if (CreatePipe(&read_end, &write_end, &sa, 0) == 0) {
        throw std::runtime_error(last_error_message(label));
    }

    PipePair pair;
    pair.read_end.reset(read_end);
    pair.write_end.reset(write_end);
    return pair;
}

bool is_stdin_closed_error(DWORD error) {
    return error == ERROR_BROKEN_PIPE || error == ERROR_NO_DATA || error == ERROR_PIPE_NOT_CONNECTED;
}

class Win32ProcessSession : public ProcessSession {
public:
    Win32ProcessSession(UniqueHandle process_handle, UniqueHandle stdin_write, UniqueHandle stdout_read)
        : process_handle_(std::move(process_handle)), stdin_write_(std::move(stdin_write)),
          stdout_read_(std::move(stdout_read)) {}

    ~Win32ProcessSession() override { terminate(); }

    void write_stdin(const std::string& chars) override {
        const char* data = chars.data();
        std::size_t remaining = chars.size();
        while (remaining > 0U) {
            DWORD written = 0;
            if (WriteFile(stdin_write_.get(), data, static_cast<DWORD>(remaining), &written, NULL) == 0) {
                const DWORD error = GetLastError();
                if (is_stdin_closed_error(error)) {
                    throw ProcessStdinClosedError(
                        "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open");
                }
                throw std::runtime_error(last_error_message("WriteFile"));
            }
            if (written == 0U) {
                throw std::runtime_error("WriteFile wrote zero bytes");
            }
            data += written;
            remaining -= static_cast<std::size_t>(written);
        }
    }

    void resize_pty(unsigned short rows, unsigned short cols) override {
        (void)rows;
        (void)cols;
        throw ProcessPtyResizeUnsupportedError("PTY resize is not supported on this host");
    }

    std::string read_output(bool block, bool* eof, std::string* carry) override {
        return read_console_output(stdout_read_.get(), block, eof, carry);
    }

    std::string flush_carry(std::string* carry) override { return flush_console_output_carry(carry); }

    bool has_exited(int* exit_code) override {
        if (!process_handle_.valid()) {
            *exit_code = 1;
            return true;
        }
        if (WaitForSingleObject(process_handle_.get(), 0) != WAIT_OBJECT_0) {
            return false;
        }
        DWORD raw_exit_code = 0;
        GetExitCodeProcess(process_handle_.get(), &raw_exit_code);
        *exit_code = static_cast<int>(raw_exit_code);
        return true;
    }

    void terminate() override {
        if (process_handle_.valid()) {
            TerminateProcess(process_handle_.get(), 1);
            process_handle_.reset();
        }
    }

private:
    UniqueHandle process_handle_;
    UniqueHandle stdin_write_;
    UniqueHandle stdout_read_;
};

} // namespace

std::unique_ptr<ProcessSession> ProcessSession::launch(
    const std::string& command, const std::string& workdir, const std::string& shell, bool login, bool tty) {
    if (tty) {
        throw std::runtime_error("tty is not supported on this host");
    }

    PipePair stdout_pipe = create_pipe_pair("CreatePipe(stdout)");
    PipePair stdin_pipe = create_pipe_pair("CreatePipe(stdin)");
    SetHandleInformation(stdout_pipe.read_end.get(), HANDLE_FLAG_INHERIT, 0);
    SetHandleInformation(stdin_pipe.write_end.get(), HANDLE_FLAG_INHERIT, 0);

    STARTUPINFOW startup_info;
    ZeroMemory(&startup_info, sizeof(startup_info));
    startup_info.cb = sizeof(startup_info);
    startup_info.dwFlags = STARTF_USESTDHANDLES;
    startup_info.hStdInput = stdin_pipe.read_end.get();
    startup_info.hStdOutput = stdout_pipe.write_end.get();
    startup_info.hStdError = stdout_pipe.write_end.get();

    PROCESS_INFORMATION process_info;
    ZeroMemory(&process_info, sizeof(process_info));

    const std::vector<std::string> argv = platform::shell_argv(shell, login, command);
    const std::string command_line = command_line_from_argv(argv);
    std::wstring wide_command_line = wide_from_utf8(command_line);
    std::vector<wchar_t> mutable_command_line(wide_command_line.begin(), wide_command_line.end());
    mutable_command_line.push_back(L'\0');
    const std::wstring wide_workdir = workdir.empty() ? std::wstring() : wide_from_utf8(workdir);

    const BOOL created = CreateProcessW(NULL,
                                        &mutable_command_line[0],
                                        NULL,
                                        NULL,
                                        TRUE,
                                        0,
                                        NULL,
                                        workdir.empty() ? NULL : wide_workdir.c_str(),
                                        &startup_info,
                                        &process_info);

    stdin_pipe.read_end.reset();
    stdout_pipe.write_end.reset();

    if (created == 0) {
        throw std::runtime_error(last_error_message("CreateProcessW"));
    }

    UniqueHandle process_handle(process_info.hProcess);
    UniqueHandle thread_handle(process_info.hThread);
    thread_handle.reset();

    return std::unique_ptr<ProcessSession>(new Win32ProcessSession(
        std::move(process_handle), std::move(stdin_pipe.write_end), std::move(stdout_pipe.read_end)));
}

bool process_session_supports_pty() {
    return false;
}

#endif
