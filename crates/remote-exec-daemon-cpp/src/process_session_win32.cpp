#ifdef _WIN32

#include <sstream>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include <winsock2.h>
#include <windows.h>

#include "console_output.h"
#include "platform.h"
#include "process_session.h"
#include "win32_error.h"
#include "win32_scoped.h"

namespace {

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

class Win32ProcessSession : public ProcessSession {
public:
    Win32ProcessSession(
        UniqueHandle process_handle,
        UniqueHandle stdin_write,
        UniqueHandle stdout_read
    ) : process_handle_(std::move(process_handle)),
        stdin_write_(std::move(stdin_write)),
        stdout_read_(std::move(stdout_read)) {}

    ~Win32ProcessSession() override {
        terminate();
    }

    void write_stdin(const std::string& chars) override {
        DWORD written = 0;
        if (WriteFile(
                stdin_write_.get(),
                chars.data(),
                static_cast<DWORD>(chars.size()),
                &written,
                NULL
            ) == 0) {
            throw std::runtime_error(last_error_message("WriteFile"));
        }
    }

    std::string read_available(std::string* carry) override {
        return read_available_console_output(stdout_read_.get(), carry);
    }

    std::string flush_carry(std::string* carry) override {
        return flush_console_output_carry(carry);
    }

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

}  // namespace

std::unique_ptr<ProcessSession> ProcessSession::launch(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login
) {
    PipePair stdout_pipe = create_pipe_pair("CreatePipe(stdout)");
    PipePair stdin_pipe = create_pipe_pair("CreatePipe(stdin)");
    SetHandleInformation(stdout_pipe.read_end.get(), HANDLE_FLAG_INHERIT, 0);
    SetHandleInformation(stdin_pipe.write_end.get(), HANDLE_FLAG_INHERIT, 0);

    STARTUPINFOA startup_info;
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

    stdin_pipe.read_end.reset();
    stdout_pipe.write_end.reset();

    if (created == 0) {
        throw std::runtime_error(last_error_message("CreateProcessA"));
    }

    UniqueHandle process_handle(process_info.hProcess);
    UniqueHandle thread_handle(process_info.hThread);
    thread_handle.reset();

    return std::unique_ptr<ProcessSession>(
        new Win32ProcessSession(
            std::move(process_handle),
            std::move(stdin_pipe.write_end),
            std::move(stdout_pipe.read_end)
        )
    );
}

#endif
