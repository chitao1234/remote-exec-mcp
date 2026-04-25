#include <algorithm>
#include <cstdlib>
#include <limits>
#include <sstream>
#include <stdexcept>
#include <vector>

#include "session_store.h"
#include "console_output.h"
#include "logging.h"
#include "win32_error.h"

namespace {

std::string make_chunk_id() {
    std::ostringstream out;
    out << "xp-" << GetTickCount() << '-' << std::rand();
    return out.str();
}

struct PipePair {
    UniqueHandle read_end;
    UniqueHandle write_end;
};

struct PollResult {
    std::string output;
    bool completed;
    DWORD exit_code;
};

const unsigned long DEFAULT_MAX_OUTPUT_TOKENS = 10000UL;
const unsigned long OMITTED_MAX_OUTPUT_TOKENS = std::numeric_limits<unsigned long>::max();
const std::size_t BYTES_PER_TOKEN = 4U;

unsigned long approximate_token_count(std::size_t bytes) {
    if (bytes == 0U) {
        return 0UL;
    }
    return static_cast<unsigned long>((bytes + BYTES_PER_TOKEN - 1U) / BYTES_PER_TOKEN);
}

unsigned long effective_max_output_tokens(unsigned long requested) {
    if (requested == OMITTED_MAX_OUTPUT_TOKENS) {
        return DEFAULT_MAX_OUTPUT_TOKENS;
    }
    return requested;
}

unsigned long count_lines(const std::string& output) {
    if (output.empty()) {
        return 0UL;
    }

    unsigned long lines = 1UL;
    for (std::string::const_iterator it = output.begin(); it != output.end(); ++it) {
        if (*it == '\n') {
            ++lines;
        }
    }
    if (output[output.size() - 1] == '\n') {
        --lines;
    }
    return lines;
}

bool is_utf8_continuation_byte(unsigned char byte) {
    return (byte & 0xC0U) == 0x80U;
}

std::size_t floor_char_boundary(const std::string& output, std::size_t max_bytes) {
    std::size_t index = std::min(max_bytes, output.size());
    while (index > 0U && index < output.size() &&
           is_utf8_continuation_byte(static_cast<unsigned char>(output[index]))) {
        --index;
    }
    return index;
}

std::size_t ceil_char_boundary(const std::string& output, std::size_t min_bytes) {
    std::size_t index = std::min(min_bytes, output.size());
    while (index < output.size() &&
           is_utf8_continuation_byte(static_cast<unsigned char>(output[index]))) {
        ++index;
    }
    return index;
}

std::size_t suffix_start_for_budget(const std::string& output, std::size_t max_bytes) {
    if (max_bytes >= output.size()) {
        return 0U;
    }
    return ceil_char_boundary(output, output.size() - max_bytes);
}

std::string truncation_prefix(unsigned long line_count) {
    std::ostringstream out;
    out << "Total output lines: " << line_count << "\n\n";
    return out.str();
}

std::string truncation_marker(unsigned long truncated_tokens) {
    std::ostringstream out;
    out << "\xE2\x80\xA6" << truncated_tokens << " tokens truncated" << "\xE2\x80\xA6";
    return out.str();
}

std::string render_output(const std::string& output, unsigned long requested_max_output_tokens) {
    if (requested_max_output_tokens == 0UL) {
        return std::string();
    }

    const std::size_t max_output_bytes =
        static_cast<std::size_t>(effective_max_output_tokens(requested_max_output_tokens)) *
        BYTES_PER_TOKEN;
    if (output.size() <= max_output_bytes) {
        return output;
    }

    const std::string prefix = truncation_prefix(count_lines(output));
    unsigned long truncated_tokens = approximate_token_count(output.size());
    for (;;) {
        const std::string marker = truncation_marker(truncated_tokens);
        if (max_output_bytes <= prefix.size() + marker.size()) {
            return prefix + marker;
        }

        const std::size_t payload_budget = max_output_bytes - prefix.size() - marker.size();
        const std::size_t head_budget = payload_budget / 2U;
        const std::size_t tail_budget = payload_budget - head_budget;
        const std::size_t head_end = floor_char_boundary(output, head_budget);
        const std::size_t tail_start =
            std::max(head_end, suffix_start_for_budget(output, tail_budget));
        const unsigned long next_truncated_tokens =
            approximate_token_count(tail_start - head_end);

        if (next_truncated_tokens == truncated_tokens) {
            return prefix + output.substr(0, head_end) + marker + output.substr(tail_start);
        }

        truncated_tokens = next_truncated_tokens;
    }
}

double wall_time_seconds(DWORD started_at_ms) {
    return static_cast<double>(GetTickCount() - started_at_ms) / 1000.0;
}

std::string read_available(const std::shared_ptr<LiveSession>& session) {
    return read_available_console_output(session->stdout_read.get(), &session->output_carry);
}

std::string flush_output_carry(const std::shared_ptr<LiveSession>& session) {
    return flush_console_output_carry(&session->output_carry);
}

Json build_response(
    const char* daemon_session_id,
    bool running,
    DWORD started_at_ms,
    bool has_exit_code,
    DWORD exit_code,
    const std::string& output,
    unsigned long max_output_tokens
) {
    const std::string trimmed = render_output(output, max_output_tokens);
    const unsigned long original_token_count = approximate_token_count(output.size());
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

std::shared_ptr<LiveSession> launch_live_session(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell
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

    stdin_pipe.read_end.reset();
    stdout_pipe.write_end.reset();

    if (created == 0) {
        throw std::runtime_error(last_error_message("CreateProcessA"));
    }

    UniqueHandle process_handle(process_info.hProcess);
    UniqueHandle thread_handle(process_info.hThread);
    thread_handle.reset();

    std::shared_ptr<LiveSession> session(new LiveSession());
    session->id = make_chunk_id();
    session->process_handle = std::move(process_handle);
    session->stdin_write = std::move(stdin_pipe.write_end);
    session->stdout_read = std::move(stdout_pipe.read_end);
    session->started_at_ms = GetTickCount();
    return session;
}

PollResult poll_session(
    const std::shared_ptr<LiveSession>& session,
    unsigned long timeout_ms
) {
    const DWORD poll_start = GetTickCount();
    std::string output;

    while (GetTickCount() - poll_start < timeout_ms) {
        output += read_available(session);

        if (WaitForSingleObject(session->process_handle.get(), 0) == WAIT_OBJECT_0) {
            output += read_available(session);
            output += flush_output_carry(session);
            DWORD exit_code = 0;
            GetExitCodeProcess(session->process_handle.get(), &exit_code);
            return PollResult{output, true, exit_code};
        }

        Sleep(25);
    }

    return PollResult{output, false, 0};
}

}  // namespace

SessionStore::SessionStore() {
    std::srand(static_cast<unsigned int>(GetTickCount()));
}

SessionStore::~SessionStore() {
    for (std::map<std::string, std::shared_ptr<LiveSession> >::iterator it = sessions_.begin();
         it != sessions_.end();
         ++it) {
        const std::shared_ptr<LiveSession>& session = it->second;
        if (session->process_handle.valid()) {
            TerminateProcess(session->process_handle.get(), 1);
        }
    }
}

Json SessionStore::start_command(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool has_yield_time_ms,
    unsigned long yield_time_ms,
    unsigned long max_output_tokens,
    const YieldTimeConfig& yield_time
) {
    {
        std::ostringstream message;
        message << "start_command cmd_preview=`" << preview_text(command, 120)
                << "` workdir=`" << workdir << "` shell=`"
                << (shell.empty() ? "cmd.exe" : shell) << '`';
        log_message(LOG_INFO, "session_store", message.str());
    }
    std::shared_ptr<LiveSession> session = launch_live_session(command, workdir, shell);

    const unsigned long timeout_ms = resolve_yield_time_ms(
        yield_time.exec_command,
        has_yield_time_ms,
        yield_time_ms
    );
    const PollResult poll_result = poll_session(session, timeout_ms);

    if (poll_result.completed) {
        Json response = build_response(
            NULL,
            false,
            session->started_at_ms,
            true,
            poll_result.exit_code,
            poll_result.output,
            max_output_tokens
        );
        {
            std::ostringstream message;
            message << "command completed before session handoff exit_code="
                    << poll_result.exit_code
                    << " output_chars=" << poll_result.output.size();
            log_message(LOG_INFO, "session_store", message.str());
        }
        return response;
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
        poll_result.output,
        max_output_tokens
    );
}

Json SessionStore::write_stdin(
    const std::string& daemon_session_id,
    const std::string& chars,
    bool has_yield_time_ms,
    unsigned long yield_time_ms,
    unsigned long max_output_tokens,
    const YieldTimeConfig& yield_time
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
                session->stdin_write.get(),
                chars.data(),
                static_cast<DWORD>(chars.size()),
                &written,
                NULL
            ) == 0) {
            throw std::runtime_error(last_error_message("WriteFile"));
        }
    }

    const YieldTimeOperationConfig& operation_config =
        chars.empty() ? yield_time.write_stdin_poll : yield_time.write_stdin_input;
    const unsigned long timeout_ms = resolve_yield_time_ms(
        operation_config,
        has_yield_time_ms,
        yield_time_ms
    );
    const PollResult poll_result = poll_session(session, timeout_ms);

    if (poll_result.completed) {
        Json response = build_response(
            NULL,
            false,
            session->started_at_ms,
            true,
            poll_result.exit_code,
            poll_result.output,
            max_output_tokens
        );
        sessions_.erase(it);
        {
            std::ostringstream message;
            message << "session completed daemon_session_id=`" << daemon_session_id
                    << "` exit_code=" << poll_result.exit_code
                    << " open_sessions=" << sessions_.size();
            log_message(LOG_INFO, "session_store", message.str());
        }
        return response;
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
        poll_result.output,
        max_output_tokens
    );
}
