#include <algorithm>
#include <atomic>
#include <cctype>
#include <cstdint>
#include <cstdlib>
#include <stdexcept>
#include <string>
#include <vector>

#include <windows.h>

#include "core/logging.h"
#include "exec/console_output.h"
#include "exec/utf8_stream_decode.h"
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

bool is_c0_control(unsigned char byte) {
    return byte <= 0x1FU || byte == 0x7FU;
}

unsigned int decode_utf8_codepoint(const std::string& utf8) {
    if (utf8.empty()) {
        return 0U;
    }

    const unsigned char first = static_cast<unsigned char>(utf8[0]);
    if ((first & 0x80U) == 0U) {
        return first;
    }
    if ((first & 0xE0U) == 0xC0U && utf8.size() >= 2U) {
        return ((first & 0x1FU) << 6U) | (static_cast<unsigned char>(utf8[1]) & 0x3FU);
    }
    if ((first & 0xF0U) == 0xE0U && utf8.size() >= 3U) {
        return ((first & 0x0FU) << 12U) |
               ((static_cast<unsigned char>(utf8[1]) & 0x3FU) << 6U) |
               (static_cast<unsigned char>(utf8[2]) & 0x3FU);
    }
    if ((first & 0xF8U) == 0xF0U && utf8.size() >= 4U) {
        return ((first & 0x07U) << 18U) |
               ((static_cast<unsigned char>(utf8[1]) & 0x3FU) << 12U) |
               ((static_cast<unsigned char>(utf8[2]) & 0x3FU) << 6U) |
               (static_cast<unsigned char>(utf8[3]) & 0x3FU);
    }
    return 0U;
}

bool is_combining_codepoint(unsigned int codepoint) {
    return (codepoint >= 0x0300U && codepoint <= 0x036FU) ||
           (codepoint >= 0x1AB0U && codepoint <= 0x1AFFU) ||
           (codepoint >= 0x1DC0U && codepoint <= 0x1DFFU) ||
           (codepoint >= 0x20D0U && codepoint <= 0x20FFU) ||
           (codepoint >= 0xFE20U && codepoint <= 0xFE2FU);
}

bool is_wide_codepoint(unsigned int codepoint) {
    if (codepoint >= 0x1100U &&
        (codepoint <= 0x115FU || codepoint == 0x2329U || codepoint == 0x232AU ||
         (codepoint >= 0x2E80U && codepoint <= 0xA4CFU && codepoint != 0x303FU) ||
         (codepoint >= 0xAC00U && codepoint <= 0xD7A3U) ||
         (codepoint >= 0xF900U && codepoint <= 0xFAFFU) ||
         (codepoint >= 0xFE10U && codepoint <= 0xFE19U) ||
         (codepoint >= 0xFE30U && codepoint <= 0xFE6FU) ||
         (codepoint >= 0xFF00U && codepoint <= 0xFF60U) ||
         (codepoint >= 0xFFE0U && codepoint <= 0xFFE6U))) {
        return true;
    }

    return codepoint >= 0x1F300U && codepoint <= 0x1FAFFU;
}

unsigned int display_width_for_codepoint(unsigned int codepoint) {
    if (codepoint == 0U) {
        return 1U;
    }
    if (is_combining_codepoint(codepoint)) {
        return 0U;
    }
    if (is_wide_codepoint(codepoint)) {
        return 2U;
    }
    return 1U;
}

std::vector<unsigned int> parse_csi_params(const std::string& raw) {
    std::vector<unsigned int> params;
    if (raw.empty()) {
        return params;
    }

    std::size_t start = 0U;
    while (start <= raw.size()) {
        const std::size_t end = raw.find(';', start);
        const std::string token = raw.substr(start, end == std::string::npos ? std::string::npos : end - start);
        if (token.empty()) {
            params.push_back(0U);
        } else {
            params.push_back(static_cast<unsigned int>(std::strtoul(token.c_str(), nullptr, 10)));
        }
        if (end == std::string::npos) {
            break;
        }
        start = end + 1U;
    }
    return params;
}

bool line_has_nonspace_char(const std::string& text) {
    for (std::size_t i = 0; i < text.size(); ++i) {
        if (text[i] != ' ') {
            return true;
        }
    }
    return false;
}

bool is_prompt_marker_char(char ch) {
    return ch == '>' || ch == '$' || ch == '#';
}

bool closed_row_should_be_joinable(const std::string& text, std::size_t physical_width) {
    return physical_width >= 80U && line_has_nonspace_char(text) && text.find_first_of(" \t") == std::string::npos;
}

std::size_t conservative_cell_count_for_utf8_text(const std::string& text) {
    std::size_t width = 0U;
    for (std::size_t index = 0U; index < text.size();) {
        const unsigned char first = static_cast<unsigned char>(text[index]);
        std::size_t length = 1U;
        if ((first & 0xE0U) == 0xC0U && index + 1U < text.size()) {
            length = 2U;
        } else if ((first & 0xF0U) == 0xE0U && index + 2U < text.size()) {
            length = 3U;
        } else if ((first & 0xF8U) == 0xF0U && index + 3U < text.size()) {
            length = 4U;
        }

        ++width;
        index += length;
    }
    return width;
}

std::string merge_joinable_rows(const std::string& previous, const std::string& current) {
    if (previous.empty()) {
        return current;
    }
    if (current.empty()) {
        return previous;
    }

    const std::size_t repeated = current.find(previous);
    if (repeated == 0U) {
        return current;
    }
    if (repeated != std::string::npos && repeated > 0U && is_prompt_marker_char(current[repeated - 1U])) {
        return current.substr(repeated);
    }

    const std::size_t max_overlap = std::min(previous.size(), current.size());
    for (std::size_t overlap = max_overlap; overlap > 0U; --overlap) {
        if (previous.compare(previous.size() - overlap, overlap, current, 0U, overlap) == 0) {
            return previous + current.substr(overlap);
        }
    }
    return previous + current;
}

std::uint64_t console_output_monotonic_ms() {
    return static_cast<std::uint64_t>(GetTickCount());
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

TerminalOutputFilter::TerminalOutputFilter()
    : state_(State::Ground), current_row_(0), current_col_(0), has_open_row_(false), open_row_(0U),
      debounce_ms_(0UL), max_hold_ms_(0UL), debounce_enabled_(false) {}

TerminalOutputFilter::TerminalOutputFilter(unsigned long debounce_ms, unsigned long max_hold_ms)
    : state_(State::Ground), current_row_(0), current_col_(0), has_open_row_(false), open_row_(0U),
      debounce_ms_(debounce_ms), max_hold_ms_(max_hold_ms), debounce_enabled_(debounce_ms > 0UL) {}

std::string TerminalOutputFilter::filter_chunk(const std::string& chunk) {
    return filter_chunk_at(chunk, console_output_monotonic_ms());
}

std::string TerminalOutputFilter::filter_chunk_at(const std::string& chunk, std::uint64_t now_ms) {
    std::string utf8_pending;
    utf8_pending.reserve(4U);

    for (std::string::const_iterator it = chunk.begin(); it != chunk.end(); ++it) {
        const unsigned char byte = static_cast<unsigned char>(*it);
        if (state_ == State::Ground && byte != 0x1BU && !is_c0_control(byte)) {
            utf8_pending.push_back(static_cast<char>(byte));
            const unsigned char first = static_cast<unsigned char>(utf8_pending[0]);
            const bool complete =
                (first & 0x80U) == 0U ||
                (utf8_pending.size() == 2U && (first & 0xE0U) == 0xC0U) ||
                (utf8_pending.size() == 3U && (first & 0xF0U) == 0xE0U) ||
                (utf8_pending.size() == 4U && (first & 0xF8U) == 0xF0U);
            if (complete) {
                emit_printable_codepoint(decode_utf8_codepoint(utf8_pending), utf8_pending);
                utf8_pending.clear();
            }
            continue;
        }

        if (!utf8_pending.empty()) {
            emit_printable_codepoint(decode_utf8_codepoint(utf8_pending), utf8_pending);
            utf8_pending.clear();
        }
        process_byte(byte);
    }

    if (!utf8_pending.empty()) {
        emit_printable_codepoint(decode_utf8_codepoint(utf8_pending), utf8_pending);
    }

    if (!debounce_enabled_) {
        return emit_touched_rows();
    }

    queue_touched_rows(now_ms);
    return emit_due_rows(now_ms);
}

std::string TerminalOutputFilter::flush_due() {
    return flush_due_at(console_output_monotonic_ms());
}

std::string TerminalOutputFilter::flush_due_at(std::uint64_t now_ms) {
    if (!debounce_enabled_) {
        return emit_touched_rows();
    }
    queue_touched_rows(now_ms);
    return emit_due_rows(now_ms);
}

std::string TerminalOutputFilter::drain_pending() {
    state_ = State::Ground;
    csi_buffer_.clear();
    std::string output = debounce_enabled_ ? emit_all_pending_rows() : emit_touched_rows();
    if (has_open_row_) {
        output.push_back('\n');
        has_open_row_ = false;
        open_row_text_.clear();
    }
    return output;
}

void TerminalOutputFilter::process_byte(unsigned char byte) {
    switch (state_) {
    case State::Ground:
        if (byte == 0x1BU) {
            state_ = State::Escape;
            return;
        }
        if (is_c0_control(byte)) {
            emit_control(byte);
        }
        return;
    case State::Escape:
        if (byte == '[') {
            state_ = State::Csi;
            csi_buffer_.clear();
            return;
        }
        if (byte == ']') {
            state_ = State::OscString;
            return;
        }
        if (byte == 'P' || byte == 'X' || byte == '^' || byte == '_') {
            state_ = State::IgnoreString;
            return;
        }
        if (byte >= 0x20U && byte <= 0x2FU) {
            state_ = State::EscapeIntermediate;
            return;
        }
        if (byte != 0x1BU) {
            state_ = State::Ground;
        }
        return;
    case State::EscapeIntermediate:
        if (byte == 0x1BU) {
            state_ = State::Escape;
            return;
        }
        if (byte >= 0x30U && byte <= 0x7EU) {
            state_ = State::Ground;
        }
        return;
    case State::Csi:
        if (byte == 0x1BU) {
            state_ = State::Escape;
            csi_buffer_.clear();
            return;
        }
        if (byte >= 0x40U && byte <= 0x7EU) {
            dispatch_csi(static_cast<char>(byte));
            state_ = State::Ground;
            csi_buffer_.clear();
            return;
        }
        if ((byte >= 0x30U && byte <= 0x3FU) || byte == ';') {
            csi_buffer_.push_back(static_cast<char>(byte));
        }
        return;
    case State::OscString:
        if (byte == 0x07U) {
            state_ = State::Ground;
            return;
        }
        if (byte == 0x1BU) {
            state_ = State::IgnoreString;
        }
        return;
    case State::IgnoreString:
        if (byte == '\\' || byte == 0x07U) {
            state_ = State::Ground;
        }
        return;
    }
}

void TerminalOutputFilter::emit_control(unsigned char byte) {
    switch (byte) {
    case '\r':
        current_col_ = 0;
        return;
    case '\n':
        if (has_open_row_ && open_row_ == static_cast<std::size_t>(current_row_)) {
            open_row_text_.clear();
        }
        {
            const std::size_t row = static_cast<std::size_t>(current_row_);
            const std::string row_text = serialize_physical_line(row);
            closed_row_text_[row] = row_text;
            closed_row_joinable_[row] = closed_row_should_be_joinable(row_text, ensure_line(row).cells.size());
        }
        current_row_ += 1;
        current_col_ = 0;
        return;
    case '\t':
        do {
            emit_explicit_space();
        } while (current_col_ % 8 != 0);
        return;
    case 0x08U:
        if (current_col_ > 0) {
            current_col_ -= 1;
        }
        return;
    default:
        return;
    }
}

void TerminalOutputFilter::emit_printable_codepoint(unsigned int codepoint, const std::string& utf8) {
    if (utf8 == " ") {
        emit_explicit_space();
        return;
    }
    write_cell_text(utf8, display_width_for_codepoint(codepoint), false);
}

void TerminalOutputFilter::emit_explicit_space() {
    write_cell_text(" ", 1U, true);
}

void TerminalOutputFilter::write_cell_text(const std::string& text, unsigned int width, bool explicit_space) {
    if (current_row_ < 0) {
        current_row_ = 0;
    }
    if (current_col_ < 0) {
        current_col_ = 0;
    }

    const std::size_t row = static_cast<std::size_t>(current_row_);
    Line& line = ensure_line(row);
    const unsigned int effective_width = width == 0U ? 1U : width;
    clear_cells_range(&line, current_col_, current_col_ + static_cast<int>(effective_width));

    if (line.cells.size() <= static_cast<std::size_t>(current_col_ + effective_width - 1U)) {
        line.cells.resize(static_cast<std::size_t>(current_col_ + effective_width));
    }

    Cell& cell = line.cells[static_cast<std::size_t>(current_col_)];
    cell.text = text;
    cell.continuation = false;
    cell.explicit_space = explicit_space;
    cell.has_text = true;
    cell.width = effective_width;

    for (unsigned int i = 1U; i < effective_width; ++i) {
        Cell& trailing = line.cells[static_cast<std::size_t>(current_col_) + i];
        trailing.text.clear();
        trailing.continuation = true;
        trailing.explicit_space = false;
        trailing.has_text = false;
        trailing.width = 0U;
    }

    touch_row(row);
    current_col_ += static_cast<int>(effective_width);
}

void TerminalOutputFilter::dispatch_csi(char action) {
    std::string params = csi_buffer_;
    bool private_mode = false;
    if (!params.empty() && params[0] == '?') {
        private_mode = true;
        params.erase(0, 1);
    }

    const std::vector<unsigned int> parsed = parse_csi_params(params);
    const unsigned int first = parsed.empty() || parsed[0] == 0U ? 1U : parsed[0];
    switch (action) {
    case 'A':
        current_row_ = std::max(0, current_row_ - static_cast<int>(first));
        return;
    case 'B':
        current_row_ += static_cast<int>(first);
        return;
    case 'C':
        current_col_ += static_cast<int>(first);
        return;
    case 'D':
        current_col_ = std::max(0, current_col_ - static_cast<int>(first));
        return;
    case 'G':
        set_cursor_col(static_cast<int>(first == 0U ? 0U : first - 1U));
        return;
    case 'H':
    case 'f': {
        const unsigned int row = parsed.size() >= 1U && parsed[0] != 0U ? parsed[0] - 1U : 0U;
        const unsigned int col = parsed.size() >= 2U && parsed[1] != 0U ? parsed[1] - 1U : 0U;
        current_row_ = static_cast<int>(row);
        set_cursor_col(static_cast<int>(col));
        return;
    }
    case 'J':
        clear_screen_from_cursor(parsed.empty() ? 0 : static_cast<int>(parsed[0]));
        return;
    case 'K':
        clear_line_from_cursor(parsed.empty() ? 0 : static_cast<int>(parsed[0]));
        return;
    case 'm':
        return;
    case 'h':
    case 'l':
        if (private_mode) {
            return;
        }
        return;
    default:
        return;
    }
}

void TerminalOutputFilter::set_cursor_col(int col) {
    current_col_ = std::max(0, col);
}

void TerminalOutputFilter::clear_line_from_cursor(int mode) {
    if (current_row_ < 0) {
        current_row_ = 0;
    }
    Line& line = ensure_line(static_cast<std::size_t>(current_row_));
    if (mode == 2) {
        line.cells.clear();
    } else if (mode == 1) {
        clear_cells_range(&line, 0, current_col_ + 1);
    } else {
        clear_cells_range(&line, current_col_, static_cast<int>(line.cells.size()));
    }
    touch_row(static_cast<std::size_t>(current_row_));
}

void TerminalOutputFilter::clear_screen_from_cursor(int mode) {
    if (mode == 2) {
        lines_.clear();
        touched_rows_.clear();
        pending_rows_.clear();
        closed_row_text_.clear();
        closed_row_joinable_.clear();
        current_row_ = 0;
        current_col_ = 0;
        has_open_row_ = false;
        open_row_text_.clear();
        return;
    }

    if (current_row_ < 0) {
        current_row_ = 0;
    }
    clear_line_from_cursor(mode == 1 ? 1 : 0);
    if (mode == 0) {
        for (std::size_t row = static_cast<std::size_t>(current_row_ + 1); row < lines_.size(); ++row) {
            lines_[row].cells.clear();
            touch_row(row);
        }
    } else if (mode == 1) {
        for (std::size_t row = 0U; row < static_cast<std::size_t>(current_row_); ++row) {
            lines_[row].cells.clear();
            touch_row(row);
        }
    }
}

void TerminalOutputFilter::touch_row(std::size_t row) {
    if (row >= lines_.size()) {
        lines_.resize(row + 1U);
    }
    if (!lines_[row].touched) {
        lines_[row].touched = true;
        touched_rows_.push_back(row);
    }
}

TerminalOutputFilter::Line& TerminalOutputFilter::ensure_line(std::size_t row) {
    if (row >= lines_.size()) {
        lines_.resize(row + 1U);
    }
    return lines_[row];
}

bool TerminalOutputFilter::line_has_any_content(const Line& line) const {
    for (std::size_t i = 0; i < line.cells.size(); ++i) {
        const Cell& cell = line.cells[i];
        if (cell.continuation) {
            continue;
        }
        if (cell.has_text || cell.explicit_space) {
            return true;
        }
    }
    return false;
}

void TerminalOutputFilter::clear_cells_range(Line* line, int start_col, int end_col) {
    if (line == nullptr) {
        return;
    }
    if (start_col < 0) {
        start_col = 0;
    }
    if (end_col <= start_col) {
        return;
    }

    int clear_start = start_col;
    int clear_end = end_col;
    if (line->cells.size() < static_cast<std::size_t>(clear_end)) {
        line->cells.resize(static_cast<std::size_t>(clear_end));
    }

    while (clear_start > 0 &&
           clear_start < static_cast<int>(line->cells.size()) &&
           line->cells[static_cast<std::size_t>(clear_start)].continuation) {
        --clear_start;
    }

    bool expanded = true;
    while (expanded) {
        expanded = false;
        if (line->cells.size() < static_cast<std::size_t>(clear_end)) {
            line->cells.resize(static_cast<std::size_t>(clear_end));
        }
        for (int col = clear_start; col < clear_end && col < static_cast<int>(line->cells.size()); ++col) {
            const Cell& cell = line->cells[static_cast<std::size_t>(col)];
            if (cell.continuation) {
                int lead = col;
                while (lead > 0 && line->cells[static_cast<std::size_t>(lead)].continuation) {
                    --lead;
                }
                if (lead < clear_start) {
                    clear_start = lead;
                    expanded = true;
                }
                const Cell& lead_cell = line->cells[static_cast<std::size_t>(lead)];
                const int cell_end = lead + static_cast<int>(lead_cell.width == 0U ? 1U : lead_cell.width);
                if (cell_end > clear_end) {
                    clear_end = cell_end;
                    expanded = true;
                }
            } else if (cell.has_text && cell.width > 1U) {
                const int cell_end = col + static_cast<int>(cell.width);
                if (cell_end > clear_end) {
                    clear_end = cell_end;
                    expanded = true;
                }
            }
        }
    }

    for (int col = clear_start; col < clear_end; ++col) {
        Cell& cell = line->cells[static_cast<std::size_t>(col)];
        cell.text.clear();
        cell.continuation = false;
        cell.explicit_space = false;
        cell.has_text = false;
        cell.width = 0U;
    }
}

std::string TerminalOutputFilter::serialize_physical_line(std::size_t row) const {
    if (row >= lines_.size()) {
        return "";
    }

    const Line& line = lines_[row];
    std::string out;
    for (std::size_t i = 0; i < line.cells.size(); ++i) {
        const Cell& cell = line.cells[i];
        if (cell.continuation) {
            continue;
        }
        if (cell.has_text) {
            out += cell.text;
        } else if (cell.explicit_space) {
            out.push_back(' ');
        }
    }

    while (!out.empty() && out[out.size() - 1] == ' ') {
        out.erase(out.size() - 1);
    }
    return out;
}

void TerminalOutputFilter::queue_touched_rows(std::uint64_t now_ms) {
    if (touched_rows_.empty()) {
        return;
    }

    std::sort(touched_rows_.begin(), touched_rows_.end());
    touched_rows_.erase(std::unique(touched_rows_.begin(), touched_rows_.end()), touched_rows_.end());

    for (std::size_t i = 0; i < touched_rows_.size(); ++i) {
        const std::size_t row = touched_rows_[i];
        if (row >= lines_.size()) {
            continue;
        }
        Line& line = lines_[row];
        PendingRow& pending = pending_rows_[row];
        if (pending.first_pending_at_ms == 0U) {
            pending.first_pending_at_ms = now_ms;
        }
        pending.last_changed_at_ms = now_ms;
        line.touched = false;
    }

    touched_rows_.clear();
}

bool TerminalOutputFilter::pending_row_due(const PendingRow& pending, std::uint64_t now_ms) const {
    if (!debounce_enabled_) {
        return true;
    }
    if (pending.last_changed_at_ms == 0U || pending.first_pending_at_ms == 0U) {
        return true;
    }
    if (now_ms - pending.last_changed_at_ms >= static_cast<std::uint64_t>(debounce_ms_)) {
        return true;
    }
    return max_hold_ms_ > 0UL && now_ms - pending.first_pending_at_ms >= static_cast<std::uint64_t>(max_hold_ms_);
}

std::string TerminalOutputFilter::emit_rows(const std::vector<std::size_t>& rows) {
    if (rows.empty()) {
        return "";
    }

    std::string output;
    for (std::size_t i = 0; i < rows.size(); ++i) {
        const std::size_t row = rows[i];
        if (row >= lines_.size()) {
            continue;
        }

        Line& line = lines_[row];
        const std::string current = serialize_physical_line(row);

        if (!line_has_any_content(line) && has_open_row_ && open_row_ == row) {
            output.push_back('\n');
            has_open_row_ = false;
            open_row_text_.clear();
            closed_row_text_.erase(row);
            closed_row_joinable_.erase(row);
            line.touched = false;
            continue;
        }

        if (!line_has_any_content(line) && !has_open_row_) {
            line.touched = false;
            continue;
        }

        const std::size_t previous_row = row == 0U ? 0U : row - 1U;
        const std::map<std::size_t, std::string>::const_iterator previous_closed = closed_row_text_.find(previous_row);
        const std::map<std::size_t, std::string>::const_iterator closed = closed_row_text_.find(row);
        const bool closed_joinable =
            closed != closed_row_text_.end() &&
            closed_row_joinable_.find(row) != closed_row_joinable_.end() &&
            closed_row_joinable_[row];
        const bool join_previous_closed_row =
            row > 0U &&
            previous_closed != closed_row_text_.end() &&
            closed_row_joinable_.find(previous_row) != closed_row_joinable_.end() &&
            closed_row_joinable_[previous_row] &&
            line_has_nonspace_char(current);
        if (join_previous_closed_row) {
            const std::string merged = merge_joinable_rows(previous_closed->second, current);
            if (has_open_row_) {
                output.push_back('\n');
                has_open_row_ = false;
                open_row_text_.clear();
            }
            output += merged;
            has_open_row_ = true;
            open_row_ = previous_row;
            open_row_text_ = merged;
            closed_row_text_.erase(previous_row);
            closed_row_joinable_.erase(previous_row);
            if (closed != closed_row_text_.end()) {
                closed_row_text_[row] = merged;
                closed_row_joinable_[row] = closed_joinable;
            }
            line.touched = false;
            continue;
        }

        if (closed != closed_row_text_.end() && !closed_joinable && closed->second == current) {
            if (has_open_row_ && open_row_ != row) {
                output.push_back('\n');
                has_open_row_ = false;
                open_row_text_.clear();
            }
            output += current;
            output.push_back('\n');
            has_open_row_ = false;
            open_row_text_.clear();
            closed_row_text_.erase(row);
            closed_row_joinable_.erase(row);
            line.touched = false;
            continue;
        }

        if (closed != closed_row_text_.end() && closed_joinable && closed->second == current) {
            line.touched = false;
            continue;
        }

        if (closed != closed_row_text_.end() && closed->second != current && line_has_nonspace_char(current)) {
            if (has_open_row_) {
                output.push_back('\n');
                has_open_row_ = false;
                open_row_text_.clear();
            }
            output += current;
            output.push_back('\n');
            closed_row_text_[row] = current;
            closed_row_joinable_[row] = false;
            line.touched = false;
            continue;
        }

        if (!line_has_nonspace_char(current)) {
            line.touched = false;
            continue;
        }

        if (has_open_row_ && open_row_ == row) {
            if (current != open_row_text_) {
                const std::string suffix = current.substr(std::min(current.size(), open_row_text_.size()));
                if (!suffix.empty()) {
                    output += suffix;
                }
                open_row_text_ = current;
            }
        } else {
            if (has_open_row_) {
                output.push_back('\n');
            }
            output += current;
            has_open_row_ = true;
            open_row_ = row;
            open_row_text_ = current;
        }

        line.touched = false;
    }

    return output;
}

std::string TerminalOutputFilter::emit_touched_rows() {
    if (touched_rows_.empty()) {
        return "";
    }

    std::sort(touched_rows_.begin(), touched_rows_.end());
    touched_rows_.erase(std::unique(touched_rows_.begin(), touched_rows_.end()), touched_rows_.end());
    const std::string output = emit_rows(touched_rows_);
    touched_rows_.clear();
    return output;
}

std::string TerminalOutputFilter::emit_due_rows(std::uint64_t now_ms) {
    if (pending_rows_.empty()) {
        return "";
    }

    std::vector<std::size_t> due_rows;
    for (std::map<std::size_t, PendingRow>::const_iterator it = pending_rows_.begin(); it != pending_rows_.end();
         ++it) {
        if (pending_row_due(it->second, now_ms)) {
            due_rows.push_back(it->first);
        }
    }
    if (due_rows.empty()) {
        return "";
    }

    const std::string output = emit_rows(due_rows);
    for (std::size_t i = 0U; i < due_rows.size(); ++i) {
        pending_rows_.erase(due_rows[i]);
    }
    return output;
}

std::string TerminalOutputFilter::emit_all_pending_rows() {
    if (!touched_rows_.empty()) {
        queue_touched_rows(console_output_monotonic_ms());
    }
    if (pending_rows_.empty()) {
        return "";
    }

    std::vector<std::size_t> rows;
    rows.reserve(pending_rows_.size());
    for (std::map<std::size_t, PendingRow>::const_iterator it = pending_rows_.begin(); it != pending_rows_.end();
         ++it) {
        rows.push_back(it->first);
    }
    const std::string output = emit_rows(rows);
    pending_rows_.clear();
    return output;
}

WinptyTranscriptNormalizer::WinptyTranscriptNormalizer() : physical_width_(120U) {}

WinptyTranscriptNormalizer::WinptyTranscriptNormalizer(std::size_t physical_width)
    : physical_width_(physical_width == 0U ? 120U : physical_width) {}

void WinptyTranscriptNormalizer::set_physical_width(std::size_t physical_width) {
    if (physical_width != 0U) {
        physical_width_ = physical_width;
    }
}

std::string WinptyTranscriptNormalizer::filter_chunk(const std::string& chunk) {
    pending_physical_line_ += chunk;

    std::string output;
    std::size_t newline = pending_physical_line_.find('\n');
    while (newline != std::string::npos) {
        std::string line = pending_physical_line_.substr(0, newline);
        pending_physical_line_.erase(0, newline + 1U);
        process_physical_line(line, &output);
        newline = pending_physical_line_.find('\n');
    }
    return output;
}

std::string WinptyTranscriptNormalizer::drain_pending() {
    std::string output;
    if (!pending_physical_line_.empty()) {
        process_physical_line(pending_physical_line_, &output);
        pending_physical_line_.clear();
    }
    if (!pending_logical_fragment_.empty()) {
        emit_logical_line(pending_logical_fragment_, &output);
        pending_logical_fragment_.clear();
    }
    return output;
}

void WinptyTranscriptNormalizer::process_physical_line(const std::string& raw_line, std::string* output) {
    if (output == nullptr) {
        return;
    }

    std::string line = raw_line;
    if (!line.empty() && line[line.size() - 1U] == '\r') {
        line.erase(line.size() - 1U);
    }

    std::string left;
    std::string right;
    if (split_winpty_repaint_line(line, &left, &right)) {
        if (!pending_logical_fragment_.empty()) {
            if (!left.empty()) {
                emit_logical_line(pending_logical_fragment_ + trim_leading_spaces(left), output);
            } else {
                emit_logical_line(pending_logical_fragment_, output);
            }
            pending_logical_fragment_.clear();
        } else if (!left.empty()) {
            emit_logical_line(left, output);
        }
        pending_logical_fragment_ = right;
        return;
    }

    if (!pending_logical_fragment_.empty()) {
        emit_logical_line(pending_logical_fragment_ + trim_leading_spaces(line), output);
        pending_logical_fragment_.clear();
        return;
    }

    emit_logical_line(line, output);
}

void WinptyTranscriptNormalizer::emit_logical_line(const std::string& line, std::string* output) {
    if (output == nullptr) {
        return;
    }
    *output += line;
    output->push_back('\n');
}

bool WinptyTranscriptNormalizer::split_winpty_repaint_line(const std::string& line,
                                                           std::string* left,
                                                           std::string* right) const {
    // WinPTY screen-scrapes console rows. When a wrapped/repainted logical line
    // is read back as one physical row, the right-edge fragment is positioned
    // by padding through the configured row width. Use that known width rather
    // than fixed gap constants.
    const std::size_t line_width = conservative_cell_count_for_utf8_text(line);
    if (line_width > physical_width_ || line_width + 1U < physical_width_) {
        return false;
    }

    for (std::size_t i = 0U; i < line.size();) {
        if (line[i] != ' ') {
            ++i;
            continue;
        }

        std::size_t end = i;
        while (end < line.size() && line[end] == ' ') {
            ++end;
        }

        const std::size_t run_len = end - i;
        if (end < line.size()) {
            const std::string candidate_left = trim_trailing_spaces(line.substr(0, i));
            const std::string candidate_right = line.substr(end);
            const std::size_t left_width = conservative_cell_count_for_utf8_text(candidate_left);
            const std::size_t right_width = conservative_cell_count_for_utf8_text(candidate_right);
            if (!candidate_right.empty() && left_width + right_width <= line_width) {
                const std::size_t right_start_col = conservative_cell_count_for_utf8_text(line.substr(0, end));
                const std::size_t expected_gap = line_width - left_width - right_width;
                const bool fills_row = run_len == expected_gap;
                const bool right_edge_fragment = right_start_col + right_width == line_width;
                const bool mostly_padding = run_len * 2U >= physical_width_;
                const bool short_repaint_fragment =
                    candidate_left.find_first_of(">$#") != std::string::npos && run_len * 4U >= physical_width_;
                if (fills_row && right_edge_fragment && (mostly_padding || short_repaint_fragment)) {
                    if (left != nullptr) {
                        *left = candidate_left;
                    }
                    if (right != nullptr) {
                        *right = candidate_right;
                    }
                    return true;
                }
            }
        }
        i = end;
    }

    return false;
}

std::string WinptyTranscriptNormalizer::trim_trailing_spaces(const std::string& text) {
    std::size_t end = text.size();
    while (end > 0U && text[end - 1U] == ' ') {
        --end;
    }
    return text.substr(0, end);
}

std::string WinptyTranscriptNormalizer::trim_leading_spaces(const std::string& text) {
    std::size_t start = 0U;
    while (start < text.size() && text[start] == ' ') {
        ++start;
    }
    return text.substr(start);
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

std::string decode_utf8_stream_for_test(std::string* carry, const std::string& raw_chunk, bool flush) {
    return utf8_stream_decode::decode_utf8_stream_chunk(carry, raw_chunk, flush);
}

std::string filter_terminal_output_for_test(TerminalOutputFilter* filter, const std::string& chunk) {
    return filter->filter_chunk(chunk);
}

std::string filter_terminal_output_at_for_test(TerminalOutputFilter* filter,
                                               const std::string& chunk,
                                               std::uint64_t now_ms) {
    return filter->filter_chunk_at(chunk, now_ms);
}

std::string flush_terminal_output_due_for_test(TerminalOutputFilter* filter, std::uint64_t now_ms) {
    return filter->flush_due_at(now_ms);
}

std::string drain_terminal_output_for_test(TerminalOutputFilter* filter) {
    return filter->drain_pending();
}

std::string normalize_winpty_transcript_chunk_for_test(WinptyTranscriptNormalizer* normalizer, const std::string& chunk) {
    return normalizer->filter_chunk(chunk);
}

std::string drain_winpty_transcript_for_test(WinptyTranscriptNormalizer* normalizer) {
    return normalizer->drain_pending();
}
#endif
