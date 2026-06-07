#pragma once

#include <cstdint>
#include <map>
#include <string>
#include <vector>

#include <windows.h>

std::string read_available_console_output(HANDLE pipe, std::string* carry);
std::string read_console_output(HANDLE pipe, bool block, bool* eof, std::string* carry);
std::string flush_console_output_carry(std::string* carry);

class TerminalOutputFilter {
public:
    TerminalOutputFilter();
    TerminalOutputFilter(unsigned long debounce_ms, unsigned long max_hold_ms);

    std::string filter_chunk(const std::string& chunk);
    std::string filter_chunk_at(const std::string& chunk, std::uint64_t now_ms);
    std::string flush_due();
    std::string flush_due_at(std::uint64_t now_ms);
    std::string drain_pending();

private:
    struct Cell {
        Cell() : continuation(false), explicit_space(false), has_text(false), width(0U) {}

        std::string text;
        bool continuation;
        bool explicit_space;
        bool has_text;
        unsigned int width;
    };

    struct Line {
        Line() : touched(false) {}

        std::vector<Cell> cells;
        bool touched;
    };

    struct PendingRow {
        PendingRow() : first_pending_at_ms(0U), last_changed_at_ms(0U) {}

        std::uint64_t first_pending_at_ms;
        std::uint64_t last_changed_at_ms;
    };

    enum class State {
        Ground,
        Escape,
        EscapeIntermediate,
        Csi,
        OscString,
        IgnoreString,
    };

    void process_byte(unsigned char byte);
    void emit_control(unsigned char byte);
    void emit_printable_codepoint(unsigned int codepoint, const std::string& utf8);
    void emit_explicit_space();
    void write_cell_text(const std::string& text, unsigned int width, bool explicit_space);
    void dispatch_csi(char action);
    void set_cursor_col(int col);
    void clear_line_from_cursor(int mode);
    void clear_screen_from_cursor(int mode);
    void touch_row(std::size_t row);
    Line& ensure_line(std::size_t row);
    bool line_has_any_content(const Line& line) const;
    void clear_cells_range(Line* line, int start_col, int end_col);
    std::string serialize_physical_line(std::size_t row) const;
    void queue_touched_rows(std::uint64_t now_ms);
    bool pending_row_due(const PendingRow& pending, std::uint64_t now_ms) const;
    std::string emit_rows(const std::vector<std::size_t>& rows);
    std::string emit_touched_rows();
    std::string emit_due_rows(std::uint64_t now_ms);
    std::string emit_all_pending_rows();

    std::vector<Line> lines_;
    std::vector<std::size_t> touched_rows_;
    std::map<std::size_t, PendingRow> pending_rows_;
    std::map<std::size_t, std::string> closed_row_text_;
    std::map<std::size_t, bool> closed_row_joinable_;

    State state_;
    std::string csi_buffer_;
    int current_row_;
    int current_col_;
    bool has_open_row_;
    std::size_t open_row_;
    std::string open_row_text_;
    unsigned long debounce_ms_;
    unsigned long max_hold_ms_;
    bool debounce_enabled_;
};

class WinptyTranscriptNormalizer {
public:
    WinptyTranscriptNormalizer();
    explicit WinptyTranscriptNormalizer(std::size_t physical_width);

    void set_physical_width(std::size_t physical_width);
    std::string filter_chunk(const std::string& chunk);
    std::string drain_pending();

private:
    enum class TrailingFragmentMode {
        Buffer,
        Emit,
    };

    void process_physical_line(const std::string& line, std::string* output);
    void process_physical_line_with_mode(
        const std::string& line,
        std::string* output,
        TrailingFragmentMode trailing_fragment_mode
    );
    void emit_repaint_prefix(const std::string& left, std::string* output);
    void emit_line_with_pending_fragment(const std::string& line, std::string* output, bool terminated);
    void emit_logical_line(const std::string& line, std::string* output);
    void emit_logical_text(const std::string& line, std::string* output, bool terminated);
    bool split_winpty_repaint_line(const std::string& line, std::string* left, std::string* right) const;
    static std::string trim_trailing_spaces(const std::string& text);
    static std::string trim_leading_spaces(const std::string& text);

    std::size_t physical_width_;
    std::string pending_physical_line_;
    std::string pending_logical_fragment_;
};

#ifdef REMOTE_EXEC_CPP_TESTING
std::string utf8_from_windows_wide_for_test(const std::wstring& wide);
std::string utf8_from_windows_code_page_for_test(unsigned int code_page, const std::string& raw);
std::string decode_console_output_for_test(
    unsigned int primary_code_page,
    unsigned int fallback_code_page,
    std::string* carry,
    const std::string& raw_chunk,
    bool flush
);
std::string decode_utf8_stream_for_test(std::string* carry, const std::string& raw_chunk, bool flush);
std::string filter_terminal_output_for_test(TerminalOutputFilter* filter, const std::string& chunk);
std::string
filter_terminal_output_at_for_test(TerminalOutputFilter* filter, const std::string& chunk, std::uint64_t now_ms);
std::string flush_terminal_output_due_for_test(TerminalOutputFilter* filter, std::uint64_t now_ms);
std::string drain_terminal_output_for_test(TerminalOutputFilter* filter);
std::string
normalize_winpty_transcript_chunk_for_test(WinptyTranscriptNormalizer* normalizer, const std::string& chunk);
std::string drain_winpty_transcript_for_test(WinptyTranscriptNormalizer* normalizer);
#endif
