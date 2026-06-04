#pragma once

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

    std::string filter_chunk(const std::string& chunk);
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
    std::string emit_touched_rows();

    std::vector<Line> lines_;
    std::vector<std::size_t> touched_rows_;
    std::map<std::size_t, std::string> closed_row_text_;
    std::map<std::size_t, bool> closed_row_joinable_;

    State state_;
    std::string csi_buffer_;
    int current_row_;
    int current_col_;
    bool has_open_row_;
    std::size_t open_row_;
    std::string open_row_text_;
};

class WinptyTranscriptNormalizer {
public:
    WinptyTranscriptNormalizer();

    std::string filter_chunk(const std::string& chunk);
    std::string drain_pending();

private:
    void process_physical_line(const std::string& line, std::string* output);
    void emit_logical_line(const std::string& line, std::string* output);
    static bool split_large_gap_line(const std::string& line, std::string* left, std::string* right);
    static std::string trim_trailing_spaces(const std::string& text);
    static std::string trim_leading_spaces(const std::string& text);

    std::string pending_physical_line_;
    std::string pending_logical_fragment_;
};

#ifdef REMOTE_EXEC_CPP_TESTING
std::string utf8_from_windows_wide_for_test(const std::wstring& wide);
std::string utf8_from_windows_code_page_for_test(unsigned int code_page, const std::string& raw);
std::string decode_console_output_for_test(unsigned int primary_code_page,
                                           unsigned int fallback_code_page,
                                           std::string* carry,
                                           const std::string& raw_chunk,
                                           bool flush);
std::string decode_utf8_stream_for_test(std::string* carry, const std::string& raw_chunk, bool flush);
std::string filter_terminal_output_for_test(TerminalOutputFilter* filter, const std::string& chunk);
std::string drain_terminal_output_for_test(TerminalOutputFilter* filter);
std::string normalize_winpty_transcript_chunk_for_test(WinptyTranscriptNormalizer* normalizer, const std::string& chunk);
std::string drain_winpty_transcript_for_test(WinptyTranscriptNormalizer* normalizer);
#endif
