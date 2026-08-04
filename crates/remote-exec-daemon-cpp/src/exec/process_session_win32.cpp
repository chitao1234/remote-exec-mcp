#ifdef _WIN32

#include <cstddef>
#include <sstream>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include <windows.h>

#ifdef REMOTE_EXEC_CPP_HAS_WINPTY
#include "winpty.h"
#endif

#include "core/logging.h"
#include "core/shell_policy_internal.h"
#include "exec/console_output.h"
#include "exec/process_session.h"
#include "exec/utf8_stream_decode.h"
#include "platform/platform.h"
#include "platform/win32_dynamic.h"
#include "platform/win32_error.h"
#include "platform/win32_native_string.h"
#include "platform/win32_process_tree.h"
#include "platform/win32_scoped.h"
#include "platform/win32_utf8.h"
#include "win32_pipe_io.h"

namespace {

using platform_detail::is_windows_cmd_family;
using platform_detail::is_windows_command_family;
using platform_detail::shell_basename_lower;

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

std::string windows_process_command_line(
    const std::string& command,
    const std::string& shell,
    bool login
) {
    const std::string lower = shell_basename_lower(shell);
    if (!is_windows_cmd_family(lower) && !is_windows_command_family(lower)) {
        return command_line_from_argv(platform::shell_argv(shell, login, command));
    }

    std::ostringstream out;
    out << windows_quote_arg(shell);
    if (!login && is_windows_cmd_family(lower)) {
        out << " /D";
    }
    if (is_windows_cmd_family(lower)) {
        out << " /S /C \"" << command << '"';
    } else {
        out << " /C";
        if (!command.empty()) {
            out << ' ' << command;
        }
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
    sa.lpSecurityDescriptor = nullptr;
    sa.bInheritHandle = TRUE;

    HANDLE read_end = nullptr;
    HANDLE write_end = nullptr;
    if (CreatePipe(&read_end, &write_end, &sa, 0) == 0) {
        throw std::runtime_error(last_error_message(label));
    }

    PipePair pair;
    pair.read_end.reset(read_end);
    pair.write_end.reset(write_end);
    return pair;
}

UniqueHandle duplicate_non_inheritable_handle(HANDLE handle, const char* label) {
    HANDLE duplicate = nullptr;
    if (DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &duplicate,
            0,
            FALSE,
            DUPLICATE_SAME_ACCESS
        )
        == 0) {
        throw std::runtime_error(last_error_message(label));
    }
    return UniqueHandle(duplicate);
}

void make_handle_non_inheritable(UniqueHandle* handle, const char* label) {
    UniqueHandle duplicate = duplicate_non_inheritable_handle(handle->get(), label);
    handle->reset(duplicate.release());
}

void write_stdin_to_pipe(const UniqueHandle& stdin_write, const std::string& chars) {
    const char* data = chars.data();
    std::size_t remaining = chars.size();
    while (remaining > 0U) {
        DWORD written = 0;
        if (WriteFile(stdin_write.get(), data, static_cast<DWORD>(remaining), &written, nullptr)
            == 0) {
            const DWORD error = GetLastError();
            if (is_win32_pipe_closed_error(error)) {
                throw ProcessStdinClosedError("stdin is closed for this session; rerun "
                                              "exec_command with tty=true to keep stdin open");
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

bool process_has_exited(const UniqueHandle& process_handle, int* exit_code) {
    if (!process_handle.valid()) {
        *exit_code = 1;
        return true;
    }
    if (WaitForSingleObject(process_handle.get(), 0) != WAIT_OBJECT_0) {
        return false;
    }
    DWORD raw_exit_code = 0;
    GetExitCodeProcess(process_handle.get(), &raw_exit_code);
    *exit_code = static_cast<int>(raw_exit_code);
    return true;
}

class Win32ProcessSession : public ProcessSession {
public:
    Win32ProcessSession(
        UniqueHandle process_handle,
        UniqueHandle stdin_write,
        UniqueHandle stdout_read
    )
        : process_handle_(std::move(process_handle)), stdin_write_(std::move(stdin_write)),
          stdout_read_(std::move(stdout_read)) {}

    ~Win32ProcessSession() override { terminate(); }

    void write_stdin(const std::string& chars) override {
        write_stdin_to_pipe(stdin_write_, chars);
    }

    void resize_pty(unsigned short rows, unsigned short cols) override {
        (void)rows;
        (void)cols;
        throw ProcessPtyResizeUnsupportedError("PTY resize requires a tty session");
    }

    std::string read_output(bool block, bool* eof, std::string* carry) override {
        return read_console_output(stdout_read_.get(), block, eof, carry);
    }

    std::string flush_carry(std::string* carry) override {
        return flush_console_output_carry(carry);
    }

    bool has_exited(int* exit_code) override {
        return process_has_exited(process_handle_, exit_code);
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

#ifdef REMOTE_EXEC_CPP_HAS_WINPTY
const unsigned short DEFAULT_PTY_ROWS = 24U;
const unsigned short DEFAULT_PTY_COLS = 120U;
const DWORD WINPTY_OPEN_TIMEOUT_MS = 10UL * 1000UL;
const unsigned long WINPTY_OUTPUT_DEBOUNCE_MS = 150UL;
const unsigned long WINPTY_OUTPUT_MAX_HOLD_MS = 500UL;
const unsigned long WINPTY_READ_POLL_MS = 25UL;

bool is_wine_runtime() {
    const HMODULE ntdll = GetModuleHandleA("ntdll.dll");
    return ntdll != nullptr && GetProcAddress(ntdll, "wine_get_version") != nullptr;
}

DWORD process_id_from_process_handle(HANDLE process) {
    typedef LONG NTSTATUS;
    typedef NTSTATUS(WINAPI * NtQueryInformationProcessFn)(HANDLE, ULONG, PVOID, ULONG, PULONG);
    struct ProcessBasicInformation {
        PVOID reserved1;
        PVOID peb_base_address;
        PVOID reserved2[2];
        ULONG_PTR unique_process_id;
        PVOID reserved3;
    };

    if (process == nullptr || process == INVALID_HANDLE_VALUE) {
        return 0U;
    }

    const HMODULE ntdll = GetModuleHandleA("ntdll.dll");
    if (ntdll == nullptr) {
        return 0U;
    }
    const NtQueryInformationProcessFn query =
        remote_exec_win32::proc_address_as<NtQueryInformationProcessFn>(
            GetProcAddress(ntdll, "NtQueryInformationProcess")
        );
    if (query == nullptr) {
        return 0U;
    }

    ProcessBasicInformation info;
    ZeroMemory(&info, sizeof(info));
    const NTSTATUS status = query(process, 0, &info, sizeof(info), nullptr);
    if (status < 0 || info.unique_process_id == 0U) {
        return 0U;
    }
    return static_cast<DWORD>(info.unique_process_id);
}

std::string utf8_from_wide(const std::wstring& wide) {
    try {
        return win32_utf8::utf8_from_wide(wide);
    } catch (const std::exception& ex) {
        throw std::runtime_error(
            std::string("unable to encode UTF-8 from Win32 wide string: ") + ex.what()
        );
    }
}

class WinptyErrorHandle {
public:
    WinptyErrorHandle() : error_(nullptr) {}
    ~WinptyErrorHandle() { reset(); }

    winpty_error_ptr_t* out_ptr() {
        reset();
        return &error_;
    }

    winpty_error_ptr_t get() const { return error_; }

    void reset() {
        if (error_ != nullptr) {
            winpty_error_free(error_);
            error_ = nullptr;
        }
    }

private:
    winpty_error_ptr_t error_;
};

std::string winpty_error_message(const char* prefix, WinptyErrorHandle* error) {
    std::ostringstream out;
    out << prefix << " failed";
    if (error->get() != nullptr) {
        out << ": " << utf8_from_wide(winpty_error_msg(error->get()));
        const DWORD code = winpty_error_code(error->get());
        if (code != WINPTY_ERROR_SUCCESS) {
            out << " (error " << code << ")";
        }
    }
    return out.str();
}

template <typename T, void (*FreeFn)(T*)> class UniqueWinptyPtr {
public:
    UniqueWinptyPtr() : ptr_(nullptr) {}
    explicit UniqueWinptyPtr(T* ptr) : ptr_(ptr) {}
    ~UniqueWinptyPtr() { reset(); }

    UniqueWinptyPtr(UniqueWinptyPtr&& other) : ptr_(other.release()) {}

    UniqueWinptyPtr& operator=(UniqueWinptyPtr&& other) {
        if (this != &other) {
            reset(other.release());
        }
        return *this;
    }

    UniqueWinptyPtr(const UniqueWinptyPtr&) = delete;
    UniqueWinptyPtr& operator=(const UniqueWinptyPtr&) = delete;

    T* get() const { return ptr_; }

    bool valid() const { return ptr_ != nullptr; }

    T* release() {
        T* released = ptr_;
        ptr_ = nullptr;
        return released;
    }

    void reset(T* ptr = nullptr) {
        if (ptr_ != nullptr) {
            FreeFn(ptr_);
        }
        ptr_ = ptr;
    }

private:
    T* ptr_;
};

typedef UniqueWinptyPtr<winpty_config_t, winpty_config_free> UniqueWinptyConfig;
typedef UniqueWinptyPtr<winpty_spawn_config_t, winpty_spawn_config_free> UniqueWinptySpawnConfig;
typedef UniqueWinptyPtr<winpty_t, winpty_free> UniqueWinpty;

UniqueWinptyConfig create_winpty_config() {
    WinptyErrorHandle error;
    UniqueWinptyConfig config(winpty_config_new(WINPTY_FLAG_COLOR_ESCAPES, error.out_ptr()));
    if (!config.valid()) {
        throw std::runtime_error(winpty_error_message("winpty_config_new", &error));
    }
    winpty_config_set_initial_size(config.get(), DEFAULT_PTY_COLS, DEFAULT_PTY_ROWS);
    winpty_config_set_mouse_mode(config.get(), WINPTY_MOUSE_MODE_NONE);
    winpty_config_set_agent_timeout(config.get(), WINPTY_OPEN_TIMEOUT_MS);
    return config;
}

UniqueWinpty open_winpty_session() {
    UniqueWinptyConfig config = create_winpty_config();
    WinptyErrorHandle error;
    UniqueWinpty winpty(winpty_open(config.get(), error.out_ptr()));
    if (!winpty.valid()) {
        throw std::runtime_error(winpty_error_message("winpty_open", &error));
    }
    return winpty;
}

UniqueHandle open_winpty_pipe(LPCWSTR name, DWORD desired_access) {
    HANDLE handle = CreateFileW(
        name,
        desired_access,
        0,
        nullptr,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        nullptr
    );
    if (handle == INVALID_HANDLE_VALUE) {
        throw std::runtime_error(last_error_message("CreateFileW"));
    }
    return UniqueHandle(handle);
}

struct SpawnedWinptyProcess {
    UniqueHandle process_handle;
    DWORD process_id;
};

SpawnedWinptyProcess spawn_winpty_process(
    winpty_t* winpty,
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login
) {
    const std::string command_line = windows_process_command_line(command, shell, login);
    std::wstring wide_command_line = win32_utf8::wide_from_utf8(command_line);
    std::wstring wide_workdir =
        workdir.empty() ? std::wstring() : win32_utf8::wide_from_utf8(workdir);

    WinptyErrorHandle error;
    UniqueWinptySpawnConfig spawn_config(winpty_spawn_config_new(
        WINPTY_SPAWN_FLAG_AUTO_SHUTDOWN | WINPTY_SPAWN_FLAG_EXIT_AFTER_SHUTDOWN,
        nullptr,
        wide_command_line.c_str(),
        workdir.empty() ? nullptr : wide_workdir.c_str(),
        nullptr,
        error.out_ptr()
    ));
    if (!spawn_config.valid()) {
        throw std::runtime_error(winpty_error_message("winpty_spawn_config_new", &error));
    }

    HANDLE process_handle = nullptr;
    HANDLE thread_handle = nullptr;
    DWORD create_process_error = 0;
    if (winpty_spawn(
            winpty,
            spawn_config.get(),
            &process_handle,
            &thread_handle,
            &create_process_error,
            error.out_ptr()
        )
        == FALSE) {
        if (error.get() != nullptr) {
            throw std::runtime_error(winpty_error_message("winpty_spawn", &error));
        }
        throw std::runtime_error(error_message_from_code("CreateProcessW", create_process_error));
    }

    UniqueHandle process(process_handle);
    UniqueHandle thread(thread_handle);
    thread.reset();

    SpawnedWinptyProcess spawned;
    spawned.process_handle = std::move(process);
    spawned.process_id = process_id_from_process_handle(spawned.process_handle.get());
    return spawned;
}

std::string read_winpty_available_raw(HANDLE pipe, bool* eof) {
    return read_pipe_available_raw(pipe, eof, true);
}

class WinptyProcessSession : public ProcessSession {
public:
    WinptyProcessSession(
        UniqueWinpty winpty,
        UniqueHandle process_handle,
        DWORD process_id,
        UniqueHandle stdin_write,
        UniqueHandle stdout_read
    )
        : winpty_(std::move(winpty)), process_handle_(std::move(process_handle)),
          process_id_(process_id), stdin_write_(std::move(stdin_write)),
          stdout_read_(std::move(stdout_read)), console_closed_(false),
          output_filter_(WINPTY_OUTPUT_DEBOUNCE_MS, WINPTY_OUTPUT_MAX_HOLD_MS) {}

    ~WinptyProcessSession() override { terminate(); }

    void write_stdin(const std::string& chars) override {
        write_stdin_to_pipe(stdin_write_, chars);
    }

    void resize_pty(unsigned short rows, unsigned short cols) override {
        if (rows == 0U || cols == 0U) {
            throw ProcessPtyResizeUnsupportedError("PTY rows and cols must be greater than zero");
        }
        WinptyErrorHandle error;
        if (winpty_set_size(
                winpty_.get(),
                static_cast<int>(cols),
                static_cast<int>(rows),
                error.out_ptr()
            )
            == FALSE) {
            throw std::runtime_error(winpty_error_message("winpty_set_size", &error));
        }
    }

    std::string read_output(bool block, bool* eof, std::string* carry) override {
        *eof = false;
        std::string output = flush_due_output();
        if (!output.empty() || !block) {
            if (!output.empty()) {
                return output;
            }
            return read_and_filter_available(eof, carry);
        }

        for (;;) {
            const std::string raw = read_winpty_available_raw(stdout_read_.get(), eof);
            if (!raw.empty() || *eof) {
                if (*eof) {
                    output += flush_carry(carry);
                    return output;
                }
                return output + filter_raw(raw, carry);
            }

            output = flush_due_output();
            if (!output.empty()) {
                return output;
            }

            platform::sleep_ms(WINPTY_READ_POLL_MS);
        }
    }

    std::string flush_carry(std::string* carry) override {
        const std::string decoded = utf8_stream_decode::decode_utf8_stream_chunk(carry, "", true);
        return output_filter_.filter_chunk(decoded) + output_filter_.drain_pending();
    }

    bool has_exited(int* exit_code) override {
        return process_has_exited(process_handle_, exit_code);
    }

    void terminate() override {
        close_console();
        (void)attempt_process_tree_termination();
        ensure_process_terminated();
    }

    bool terminate_descendants() override {
        close_console();
        return attempt_process_tree_termination();
    }

private:
    std::string filter_raw(const std::string& raw, std::string* carry) {
        const std::string decoded = utf8_stream_decode::decode_utf8_stream_chunk(carry, raw, false);
        return output_filter_.filter_chunk(decoded);
    }

    std::string read_and_filter_available(bool* eof, std::string* carry) {
        return filter_raw(read_winpty_available_raw(stdout_read_.get(), eof), carry)
               + flush_due_output();
    }

    std::string flush_due_output() { return output_filter_.flush_due(); }

    void close_console() {
        if (console_closed_) {
            return;
        }
        stdin_write_.reset();
        winpty_.reset();
        console_closed_ = true;
    }

    bool attempt_process_tree_termination() {
        if (!process_handle_.valid() || process_id_ == 0U) {
            return false;
        }
        return win32_process_tree::terminate_process_descendants(process_id_);
    }

    void ensure_process_terminated() {
        if (!process_handle_.valid()
            || WaitForSingleObject(process_handle_.get(), 0) == WAIT_OBJECT_0) {
            return;
        }
        TerminateProcess(process_handle_.get(), 1);
    }

    UniqueWinpty winpty_;
    UniqueHandle process_handle_;
    DWORD process_id_;
    UniqueHandle stdin_write_;
    UniqueHandle stdout_read_;
    bool console_closed_;
    TerminalOutputFilter output_filter_;
};

std::unique_ptr<ProcessSession> launch_winpty_process_session(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login
) {
    UniqueWinpty winpty = open_winpty_session();
    UniqueHandle stdin_write(open_winpty_pipe(winpty_conin_name(winpty.get()), GENERIC_WRITE));
    UniqueHandle stdout_read(open_winpty_pipe(winpty_conout_name(winpty.get()), GENERIC_READ));
    SpawnedWinptyProcess process =
        spawn_winpty_process(winpty.get(), command, workdir, shell, login);

    return std::unique_ptr<ProcessSession>(new WinptyProcessSession(
        std::move(winpty),
        std::move(process.process_handle),
        process.process_id,
        std::move(stdin_write),
        std::move(stdout_read)
    ));
}
#endif

} // namespace

std::unique_ptr<ProcessSession> ProcessSession::launch(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login,
    bool tty
) {
    if (tty) {
#ifdef REMOTE_EXEC_CPP_HAS_WINPTY
        if (!process_session_supports_pty()) {
            throw std::runtime_error("tty is not supported on this host");
        }
        return launch_winpty_process_session(command, workdir, shell, login);
#else
        throw std::runtime_error("tty is not supported on this host");
#endif
    }

    PipePair stdout_pipe = create_pipe_pair("CreatePipe(stdout)");
    PipePair stdin_pipe = create_pipe_pair("CreatePipe(stdin)");
    make_handle_non_inheritable(&stdout_pipe.read_end, "DuplicateHandle(stdout)");
    make_handle_non_inheritable(&stdin_pipe.write_end, "DuplicateHandle(stdin)");

    remote_exec_win32::NativeStartupInfo startup_info;
    ZeroMemory(&startup_info, sizeof(startup_info));
    startup_info.cb = sizeof(startup_info);
    startup_info.dwFlags = STARTF_USESTDHANDLES;
    startup_info.hStdInput = stdin_pipe.read_end.get();
    startup_info.hStdOutput = stdout_pipe.write_end.get();
    startup_info.hStdError = stdout_pipe.write_end.get();

    PROCESS_INFORMATION process_info;
    ZeroMemory(&process_info, sizeof(process_info));

    const std::string command_line = windows_process_command_line(command, shell, login);
    remote_exec_win32::NativeString native_command_line =
        remote_exec_win32::native_from_utf8(command_line, "CreateProcess");
    std::vector<remote_exec_win32::NativeChar> mutable_command_line(
        native_command_line.begin(),
        native_command_line.end()
    );
    mutable_command_line.push_back(remote_exec_win32::native_char('\0'));
    const remote_exec_win32::NativeString native_workdir =
        workdir.empty() ? remote_exec_win32::NativeString()
                        : remote_exec_win32::native_from_utf8(workdir, "CreateProcess");

    const BOOL created = remote_exec_win32::create_process_native(
        nullptr,
        &mutable_command_line[0],
        nullptr,
        nullptr,
        TRUE,
        0,
        nullptr,
        workdir.empty() ? nullptr : native_workdir.c_str(),
        &startup_info,
        &process_info
    );

    stdin_pipe.read_end.reset();
    stdout_pipe.write_end.reset();

    if (created == 0) {
        throw std::runtime_error(
            last_error_message(remote_exec_win32::native_api_name("CreateProcess").c_str())
        );
    }

    UniqueHandle process_handle(process_info.hProcess);
    UniqueHandle thread_handle(process_info.hThread);
    thread_handle.reset();

    return std::unique_ptr<ProcessSession>(new Win32ProcessSession(
        std::move(process_handle),
        std::move(stdin_pipe.write_end),
        std::move(stdout_pipe.read_end)
    ));
}

bool process_session_supports_pty() {
#ifdef REMOTE_EXEC_CPP_HAS_WINPTY
    static const bool supported = []() {
        if (is_wine_runtime()) {
            return false;
        }
        try {
            UniqueWinpty probe = open_winpty_session();
            return probe.valid();
        } catch (const std::exception& ex) {
            log_message(
                LOG_WARN,
                "process_session",
                std::string("WinPTY probe failed; tty support disabled: ") + ex.what()
            );
            return false;
        }
    }();
    return supported;
#else
    return false;
#endif
}

#ifdef REMOTE_EXEC_CPP_TESTING
std::string windows_process_command_line_for_test(
    const std::string& command,
    const std::string& shell,
    bool login
) {
    return windows_process_command_line(command, shell, login);
}
#endif

#endif
