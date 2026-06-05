#include "test_assert.h"

#include <stdexcept>
#include <string>

#include <windows.h>

#include "exec/console_output.h"
#include "platform/win32_utf8.h"

namespace {

std::wstring wide_from_units(const unsigned int* units, std::size_t count) {
    std::wstring wide;
    for (std::size_t index = 0; index < count; ++index) {
        wide.push_back(static_cast<wchar_t>(units[index]));
    }
    return wide;
}

bool code_page_available(unsigned int code_page) {
    const std::string ascii = "x";
    try {
        return utf8_from_windows_code_page_for_test(code_page, ascii) == ascii;
    } catch (const std::exception&) {
        return false;
    }
}

std::string winpty_repaint_physical_line(const std::string& left, const std::string& right, std::size_t width) {
    TEST_ASSERT(left.size() + right.size() <= width);
    return left + std::string(width - left.size() - right.size(), ' ') + right + "\n";
}

void assert_code_page_decode(unsigned int code_page, const std::string& raw, const std::string& expected_utf8) {
    if (!code_page_available(code_page)) {
        return;
    }
    TEST_ASSERT(utf8_from_windows_code_page_for_test(code_page, raw) == expected_utf8);
}

void test_wide_to_utf8() {
    TEST_ASSERT(utf8_from_windows_wide_for_test(L"ascii") == "ascii");

    const unsigned int cjk_units[] = {0x4F60U, 0x597DU};
    TEST_ASSERT(utf8_from_windows_wide_for_test(wide_from_units(cjk_units, 2U)) == "\xE4\xBD\xA0\xE5\xA5\xBD");

    const unsigned int surrogate_pair[] = {0xD83DU, 0xDE00U};
    TEST_ASSERT(utf8_from_windows_wide_for_test(wide_from_units(surrogate_pair, 2U)) == "\xF0\x9F\x98\x80");

    const unsigned int unpaired_high[] = {'x', 0xD83DU, 'y'};
    TEST_ASSERT(utf8_from_windows_wide_for_test(wide_from_units(unpaired_high, 3U)) == "x\xEF\xBF\xBD"
                                                                                       "y");

    const unsigned int unpaired_low[] = {'x', 0xDE00U, 'y'};
    TEST_ASSERT(utf8_from_windows_wide_for_test(wide_from_units(unpaired_low, 3U)) == "x\xEF\xBF\xBD"
                                                                                      "y");
}

void test_utf8_to_wide() {
    TEST_ASSERT(win32_utf8::wide_from_utf8("ascii") == L"ascii");

    const std::wstring cjk = win32_utf8::wide_from_utf8("\xE4\xBD\xA0\xE5\xA5\xBD");
    TEST_ASSERT(cjk.size() == 2U);
    TEST_ASSERT(static_cast<unsigned int>(cjk[0]) == 0x4F60U);
    TEST_ASSERT(static_cast<unsigned int>(cjk[1]) == 0x597DU);

    const std::wstring supplementary = win32_utf8::wide_from_utf8("\xF0\x9F\x98\x80");
    TEST_ASSERT(supplementary.size() == 2U);
    TEST_ASSERT(static_cast<unsigned int>(supplementary[0]) == 0xD83DU);
    TEST_ASSERT(static_cast<unsigned int>(supplementary[1]) == 0xDE00U);
}

void assert_invalid_utf8(const std::string& value) {
    bool rejected = false;
    try {
        (void)win32_utf8::wide_from_utf8(value);
    } catch (const std::exception&) {
        rejected = true;
    }
    TEST_ASSERT(rejected);
}

void test_invalid_utf8_to_wide() {
    assert_invalid_utf8("\x80");
    assert_invalid_utf8("\xC0\xAF");
    assert_invalid_utf8("\xE0\x80\xAF");
    assert_invalid_utf8("\xED\xA0\x80");
    assert_invalid_utf8("\xF4\x90\x80\x80");
    assert_invalid_utf8("\xE4\xBD");
}

void test_code_page_decode() {
    assert_code_page_decode(936U, "\xC4\xE3\xBA\xC3", "\xE4\xBD\xA0\xE5\xA5\xBD");
    assert_code_page_decode(932U, "\x82\xB1\x82\xF1\x82\xC9\x82\xBF\x82\xCD", "\xE3\x81\x93\xE3\x82\x93"
                                                                               "\xE3\x81\xAB\xE3\x81\xA1"
                                                                               "\xE3\x81\xAF");
    assert_code_page_decode(950U, "\xA7\x41\xA6\x6E", "\xE4\xBD\xA0\xE5\xA5\xBD");
    assert_code_page_decode(949U, "\xC7\xD1\xB1\xDB", "\xED\x95\x9C\xEA\xB8\x80");
}

void assert_decode_split_carry(unsigned int code_page,
                               const std::string& first_byte,
                               const std::string& second_byte,
                               const std::string& expected_utf8) {
    if (!code_page_available(code_page)) {
        return;
    }

    std::string carry;
    TEST_ASSERT(decode_console_output_for_test(code_page, 1252U, &carry, first_byte, false).empty());
    TEST_ASSERT(carry == first_byte);
    TEST_ASSERT(decode_console_output_for_test(code_page, 1252U, &carry, second_byte, false) == expected_utf8);
    TEST_ASSERT(carry.empty());
}

void test_decode_carry() {
    assert_decode_split_carry(936U, "\xC4", "\xE3", "\xE4\xBD\xA0");
    assert_decode_split_carry(932U, "\x82", "\xB1", "\xE3\x81\x93");
    assert_decode_split_carry(950U, "\xA4", "\xA4", "\xE4\xB8\xAD");
    assert_decode_split_carry(949U, "\xC7", "\xD1", "\xED\x95\x9C");
}

void assert_decode_without_false_carry(unsigned int code_page,
                                       const std::string& raw,
                                       const std::string& expected_utf8) {
    if (!code_page_available(code_page)) {
        return;
    }

    std::string carry;
    TEST_ASSERT(decode_console_output_for_test(code_page, 1252U, &carry, raw, false) == expected_utf8);
    TEST_ASSERT(carry.empty());
}

void test_decode_complete_dbcs_character_does_not_carry_trail_byte() {
    assert_decode_without_false_carry(936U, "\xC4\xE3", "\xE4\xBD\xA0");
    assert_decode_without_false_carry(932U, "\x93\xFA", "\xE6\x97\xA5");
    assert_decode_without_false_carry(950U, "\xA4\xA4", "\xE4\xB8\xAD");
    assert_decode_without_false_carry(949U, "\xC7\xD1", "\xED\x95\x9C");
}

void test_invalid_decode_fallback() {
    std::string carry;
    TEST_ASSERT(decode_console_output_for_test(99999U, 99999U, &carry, "ok\xFF", true) == "ok\xEF\xBF\xBD");
    TEST_ASSERT(carry.empty());
}

void test_utf8_stream_decode_carry() {
    std::string carry;
    TEST_ASSERT(decode_utf8_stream_for_test(&carry, "\xED\x95", false).empty());
    TEST_ASSERT(carry == "\xED\x95");
    TEST_ASSERT(decode_utf8_stream_for_test(&carry, "\x9C", false) == "\xED\x95\x9C");
    TEST_ASSERT(carry.empty());
}

void test_utf8_stream_decode_flush_replaces_incomplete_suffix() {
    std::string carry;
    TEST_ASSERT(decode_utf8_stream_for_test(&carry, "\xED\x95", false).empty());
    TEST_ASSERT(carry == "\xED\x95");
    TEST_ASSERT(decode_utf8_stream_for_test(&carry, "", true) == "\xEF\xBF\xBD");
    TEST_ASSERT(carry.empty());
}

void test_terminal_output_filter_strips_winpty_control_sequences() {
    TerminalOutputFilter filter;
    TEST_ASSERT(filter_terminal_output_for_test(
                    &filter, "\x1b[0m\x1b[0Khello\x1b[0K\x1b[?25l\r\n\x1b[0K\x1b[?25h") == "hello\n");
    TEST_ASSERT(drain_terminal_output_for_test(&filter).empty());
}

void test_terminal_output_filter_strips_osc_title_sequences() {
    TerminalOutputFilter filter;
    TEST_ASSERT(filter_terminal_output_for_test(
                    &filter, "\x1b]0;C:\\Windows\\system32\\cmd.exe\x07hello \r\n") == "hello\n");
    TEST_ASSERT(drain_terminal_output_for_test(&filter).empty());
}

void test_terminal_output_filter_handles_split_escape_sequences() {
    TerminalOutputFilter filter;
    TEST_ASSERT(filter_terminal_output_for_test(&filter, "before\x1b[") == "before");
    TEST_ASSERT(filter_terminal_output_for_test(&filter, "0Kafter") == "after");
    TEST_ASSERT(drain_terminal_output_for_test(&filter) == "\n");
}

void test_terminal_output_filter_applies_backspace_to_utf8_codepoints() {
    TerminalOutputFilter filter;
    TEST_ASSERT(filter_terminal_output_for_test(&filter, "\xE4\xBD\xA0\xE5\xA5\xBD\x08!\r\n") == "\xE4\xBD\xA0!\n");
}

void test_terminal_output_filter_collapses_winpty_prompt_rewrites() {
    TerminalOutputFilter filter;
    const std::string first =
        filter_terminal_output_for_test(&filter,
                                        "Microsoft Windows XP [\xE7\x89\x88\xE6\x9C\xAC 5.1.2600]"
                                        "\x1b[105G(C\r\n)\x20"
                                        "\xE7\x89\x88\xE6\x9D\x83\xE6\x89\x80\xE6\x9C\x89 1985-2001 Microsoft Corp.\r\n"
                                        "\x1b[107GC:\\chi\r\n>\r");
    const std::string second =
        filter_terminal_output_for_test(&filter,
                                        "\x1b[107GC:\\chi>e\r\n"
                                        "cho hello\x1b[100Ghello\r\n"
                                        "\x1b[107GC:\\chi>\r");
    const std::string final = first + second + drain_terminal_output_for_test(&filter);

    TEST_ASSERT(final.find("Microsoft Windows XP [\xE7\x89\x88\xE6\x9C\xAC 5.1.2600]") != std::string::npos);
    TEST_ASSERT(final.find("\xE7\x89\x88\xE6\x9D\x83\xE6\x89\x80\xE6\x9C\x89 1985-2001 Microsoft Corp.") != std::string::npos);
    TEST_ASSERT(final.find("C:\\chi>") != std::string::npos);
    TEST_ASSERT(final.find("hellohello") != std::string::npos);
    TEST_ASSERT(final.find("                                                                ") == std::string::npos);
    TEST_ASSERT(final.find('\x1b') == std::string::npos);
}

void test_winpty_transcript_normalizer_reconstructs_banner_and_prompt() {
    WinptyTranscriptNormalizer normalizer;
    const std::string physical =
        "Microsoft Windows XP [\xE7\x89\x88\xE6\x9C\xAC 5.1.2600]"
        "                                                                                    (C\n"
        ") \xE7\x89\x88\xE6\x9D\x83\xE6\x89\x80\xE6\x9C\x89 1985-2001 Microsoft Corp.\n"
        "                                                                                                                  C:\\chi\n"
        "\\winpty-probe>\n";
    const std::string expected =
        "Microsoft Windows XP [\xE7\x89\x88\xE6\x9C\xAC 5.1.2600]\n"
        "(C) \xE7\x89\x88\xE6\x9D\x83\xE6\x89\x80\xE6\x9C\x89 1985-2001 Microsoft Corp.\n"
        "C:\\chi\\winpty-probe>\n";

    const std::string actual = normalize_winpty_transcript_chunk_for_test(&normalizer, physical) +
                               drain_winpty_transcript_for_test(&normalizer);
    TEST_ASSERT(actual == expected);
}

void test_winpty_transcript_normalizer_splits_echo_output_from_prompt_repaint() {
    WinptyTranscriptNormalizer normalizer;
    const std::string physical =
        "\\winpty-probe>echo hello                                                                                          hello\n"
        "                                                                                                                  C:\\chi\n"
        "\\winpty-probe>\n";
    const std::string expected =
        "\\winpty-probe>echo hello\n"
        "hello\n"
        "C:\\chi\\winpty-probe>\n";

    const std::string actual = normalize_winpty_transcript_chunk_for_test(&normalizer, physical) +
                               drain_winpty_transcript_for_test(&normalizer);
    TEST_ASSERT(actual == expected);
}

void test_winpty_transcript_normalizer_handles_chunked_repaint_lines() {
    WinptyTranscriptNormalizer normalizer;
    std::string actual;
    actual += normalize_winpty_transcript_chunk_for_test(
        &normalizer, "\\winpty-probe>echo hello                                                ");
    actual += normalize_winpty_transcript_chunk_for_test(
        &normalizer, "                                          hello\n"
                     "                                                   ");
    actual += normalize_winpty_transcript_chunk_for_test(
        &normalizer, "                                                               C:\\chi\n\\winpty");
    actual += normalize_winpty_transcript_chunk_for_test(&normalizer, "-probe>\n");
    actual += drain_winpty_transcript_for_test(&normalizer);

    const std::string expected =
        "\\winpty-probe>echo hello\n"
        "hello\n"
        "C:\\chi\\winpty-probe>\n";
    TEST_ASSERT(actual == expected);
}

void test_winpty_transcript_normalizer_handles_windows_2000_short_wrap_fragment() {
    WinptyTranscriptNormalizer normalizer;
    const std::string user_profile = "DOCUME~1\\REMOTE~1\\LOCALS~1\\Temp\\remote-exec-cpp-session-store-test-probe";
    const std::string wrapped_prompt_fragment = user_profile.substr(1U) + ">";
    const std::string full_prompt = "C:\\" + user_profile + ">";
    const std::string command_line = wrapped_prompt_fragment + "echo hello";
    const std::string physical = winpty_repaint_physical_line(command_line, "hell", 120U) + "o\n" + full_prompt + "\n";
    const std::string expected =
        command_line + "\n"
        "hello\n" +
        full_prompt + "\n";

    const std::string actual = normalize_winpty_transcript_chunk_for_test(&normalizer, physical) +
                               drain_winpty_transcript_for_test(&normalizer);
    TEST_ASSERT(actual == expected);
}

void test_winpty_transcript_normalizer_merges_pending_repaint_fragment() {
    WinptyTranscriptNormalizer normalizer;
    const std::string physical =
        "                                                                                                                  C:\\chi\n"
        "\\winpty-probe>\n";
    const std::string expected = "C:\\chi\\winpty-probe>\n";

    const std::string actual = normalize_winpty_transcript_chunk_for_test(&normalizer, physical) +
                               drain_winpty_transcript_for_test(&normalizer);
    TEST_ASSERT(actual == expected);
}

void test_winpty_transcript_normalizer_preserves_regular_wide_spacing() {
    WinptyTranscriptNormalizer normalizer;
    const std::string physical =
        "column-a                                                column-b                                 column-c\n";

    const std::string actual = normalize_winpty_transcript_chunk_for_test(&normalizer, physical) +
                               drain_winpty_transcript_for_test(&normalizer);
    TEST_ASSERT(actual == physical);
}

} // namespace

int main() {
    test_wide_to_utf8();
    test_utf8_to_wide();
    test_invalid_utf8_to_wide();
    test_code_page_decode();
    test_decode_carry();
    test_decode_complete_dbcs_character_does_not_carry_trail_byte();
    test_invalid_decode_fallback();
    test_utf8_stream_decode_carry();
    test_utf8_stream_decode_flush_replaces_incomplete_suffix();
    test_terminal_output_filter_strips_winpty_control_sequences();
    test_terminal_output_filter_strips_osc_title_sequences();
    test_terminal_output_filter_handles_split_escape_sequences();
    test_terminal_output_filter_applies_backspace_to_utf8_codepoints();
    test_terminal_output_filter_collapses_winpty_prompt_rewrites();
    test_winpty_transcript_normalizer_reconstructs_banner_and_prompt();
    test_winpty_transcript_normalizer_splits_echo_output_from_prompt_repaint();
    test_winpty_transcript_normalizer_handles_chunked_repaint_lines();
    test_winpty_transcript_normalizer_handles_windows_2000_short_wrap_fragment();
    test_winpty_transcript_normalizer_merges_pending_repaint_fragment();
    test_winpty_transcript_normalizer_preserves_regular_wide_spacing();
    return 0;
}
