#include "test_assert.h"

#include <cstdio>
#include <stdexcept>
#include <string>
#include <vector>

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

std::string raw_from_code_page(unsigned int code_page, const std::wstring& wide) {
    if (wide.empty()) {
        return "";
    }

    const int length = WideCharToMultiByte(
        code_page,
        0,
        wide.data(),
        static_cast<int>(wide.size()),
        nullptr,
        0,
        nullptr,
        nullptr
    );
    if (length <= 0) {
        throw std::runtime_error("WideCharToMultiByte length failed");
    }

    std::string raw;
    raw.resize(static_cast<std::size_t>(length));
    if (WideCharToMultiByte(
            code_page,
            0,
            wide.data(),
            static_cast<int>(wide.size()),
            &raw[0],
            length,
            nullptr,
            nullptr
        )
        <= 0) {
        throw std::runtime_error("WideCharToMultiByte failed");
    }
    return raw;
}

bool is_wine_runtime() {
    const HMODULE ntdll = GetModuleHandleA("ntdll.dll");
    return ntdll != nullptr && GetProcAddress(ntdll, "wine_get_version") != nullptr;
}

bool should_skip_real_winpty_timing_tests() {
#ifdef REMOTE_EXEC_CPP_HAS_WINPTY
    if (is_wine_runtime()) {
        static bool warned = false;
        if (!warned) {
            std::fprintf(stderr, "warning: skipping WinPTY timing tests under Wine\n");
            warned = true;
        }
        return true;
    }
    return false;
#else
    return true;
#endif
}

void assert_code_page_decode(
    unsigned int code_page,
    const std::string& raw,
    const std::string& expected_utf8
) {
    if (!code_page_available(code_page)) {
        return;
    }
    TEST_ASSERT(utf8_from_windows_code_page_for_test(code_page, raw) == expected_utf8);
}

void test_wide_to_utf8() {
    TEST_ASSERT(utf8_from_windows_wide_for_test(L"ascii") == "ascii");

    const unsigned int cjk_units[] = {0x4F60U, 0x597DU};
    TEST_ASSERT(
        utf8_from_windows_wide_for_test(wide_from_units(cjk_units, 2U))
        == "\xE4\xBD\xA0\xE5\xA5\xBD"
    );

    const unsigned int surrogate_pair[] = {0xD83DU, 0xDE00U};
    TEST_ASSERT(
        utf8_from_windows_wide_for_test(wide_from_units(surrogate_pair, 2U)) == "\xF0\x9F\x98\x80"
    );

    const unsigned int unpaired_high[] = {'x', 0xD83DU, 'y'};
    TEST_ASSERT(
        utf8_from_windows_wide_for_test(wide_from_units(unpaired_high, 3U))
        == "x\xEF\xBF\xBD"
           "y"
    );

    const unsigned int unpaired_low[] = {'x', 0xDE00U, 'y'};
    TEST_ASSERT(
        utf8_from_windows_wide_for_test(wide_from_units(unpaired_low, 3U))
        == "x\xEF\xBF\xBD"
           "y"
    );
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
    assert_code_page_decode(
        932U,
        "\x82\xB1\x82\xF1\x82\xC9\x82\xBF\x82\xCD",
        "\xE3\x81\x93\xE3\x82\x93"
        "\xE3\x81\xAB\xE3\x81\xA1"
        "\xE3\x81\xAF"
    );
    assert_code_page_decode(950U, "\xA7\x41\xA6\x6E", "\xE4\xBD\xA0\xE5\xA5\xBD");
    assert_code_page_decode(949U, "\xC7\xD1\xB1\xDB", "\xED\x95\x9C\xEA\xB8\x80");
}

void assert_decode_split_carry(
    unsigned int code_page,
    const std::string& first_byte,
    const std::string& second_byte,
    const std::string& expected_utf8
) {
    if (!code_page_available(code_page)) {
        return;
    }

    std::string carry;
    TEST_ASSERT(decode_console_output_for_test(code_page, 1252U, &carry, first_byte, false).empty()
    );
    TEST_ASSERT(carry == first_byte);
    TEST_ASSERT(
        decode_console_output_for_test(code_page, 1252U, &carry, second_byte, false)
        == expected_utf8
    );
    TEST_ASSERT(carry.empty());
}

void assert_decode_split_chunks(
    unsigned int code_page,
    const std::vector<std::string>& chunks,
    const std::string& expected_utf8
) {
    if (!code_page_available(code_page)) {
        return;
    }
    TEST_ASSERT(!chunks.empty());

    std::string carry;
    for (std::size_t index = 0; index + 1U < chunks.size(); ++index) {
        TEST_ASSERT(
            decode_console_output_for_test(code_page, 1252U, &carry, chunks[index], false).empty()
        );
        TEST_ASSERT(!carry.empty());
    }

    TEST_ASSERT(
        decode_console_output_for_test(code_page, 1252U, &carry, chunks.back(), false)
        == expected_utf8
    );
    TEST_ASSERT(carry.empty());
}

void test_decode_carry() {
    assert_decode_split_carry(936U, "\xC4", "\xE3", "\xE4\xBD\xA0");
    assert_decode_split_carry(932U, "\x82", "\xB1", "\xE3\x81\x93");
    assert_decode_split_carry(950U, "\xA4", "\xA4", "\xE4\xB8\xAD");
    assert_decode_split_carry(949U, "\xC7", "\xD1", "\xED\x95\x9C");
}

void test_decode_carries_split_utf8_code_page() {
    assert_decode_split_chunks(65001U, {"\xE4\xBD", "\xA0"}, "\xE4\xBD\xA0");
    assert_decode_split_chunks(65001U, {"\xF0", "\x9F\x98", "\x80"}, "\xF0\x9F\x98\x80");
}

void test_decode_carries_split_four_byte_code_page_sequence() {
    if (!code_page_available(54936U)) {
        return;
    }

    const unsigned int surrogate_pair[] = {0xD83DU, 0xDE00U};
    std::string raw;
    try {
        raw = raw_from_code_page(54936U, wide_from_units(surrogate_pair, 2U));
    } catch (const std::exception&) {
        return;
    }
    if (raw.size() != 4U) {
        return;
    }

    assert_decode_split_chunks(
        54936U,
        {raw.substr(0U, 2U), raw.substr(2U, 1U), raw.substr(3U)},
        "\xF0\x9F\x98\x80"
    );
}

void assert_decode_without_false_carry(
    unsigned int code_page,
    const std::string& raw,
    const std::string& expected_utf8
) {
    if (!code_page_available(code_page)) {
        return;
    }

    std::string carry;
    TEST_ASSERT(
        decode_console_output_for_test(code_page, 1252U, &carry, raw, false) == expected_utf8
    );
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
    if (code_page_available(1252U)) {
        TEST_ASSERT(
            decode_console_output_for_test(99999U, 1252U, &carry, "caf\xE9", true) == "caf\xC3\xA9"
        );
        TEST_ASSERT(carry.empty());
    }

    TEST_ASSERT(decode_console_output_for_test(99999U, 99999U, &carry, "\xE4\xBD", false).empty());
    TEST_ASSERT(carry == "\xE4\xBD");
    TEST_ASSERT(
        decode_console_output_for_test(99999U, 99999U, &carry, "\xA0 ok\xFF", true)
        == "\xE4\xBD\xA0"
           " ok\xEF\xBF\xBD"
    );
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

void test_utf8_stream_decode_replaces_invalid_scalar_sequences() {
    std::string carry;
    TEST_ASSERT(
        decode_utf8_stream_for_test(&carry, "\xE0\x80\x80", false)
        == "\xEF\xBF\xBD"
           "\xEF\xBF\xBD"
           "\xEF\xBF\xBD"
    );
    TEST_ASSERT(carry.empty());
    TEST_ASSERT(
        decode_utf8_stream_for_test(&carry, "\xED\xA0\x80", false)
        == "\xEF\xBF\xBD"
           "\xEF\xBF\xBD"
           "\xEF\xBF\xBD"
    );
    TEST_ASSERT(carry.empty());
    TEST_ASSERT(
        decode_utf8_stream_for_test(&carry, "\xF4\x90\x80\x80", false)
        == "\xEF\xBF\xBD"
           "\xEF\xBF\xBD"
           "\xEF\xBF\xBD"
           "\xEF\xBF\xBD"
    );
    TEST_ASSERT(carry.empty());
}

void test_terminal_output_filter_strips_winpty_control_sequences() {
    TerminalOutputFilter filter;
    TEST_ASSERT(
        filter_terminal_output_for_test(
            &filter,
            "\x1b[0m\x1b[0Khello\x1b[0K\x1b[?25l\r\n\x1b[0K\x1b[?25h"
        )
        == "hello\n"
    );
    TEST_ASSERT(drain_terminal_output_for_test(&filter).empty());
}

void test_terminal_output_filter_strips_osc_title_sequences() {
    TerminalOutputFilter filter;
    TEST_ASSERT(
        filter_terminal_output_for_test(
            &filter,
            "\x1b]0;C:\\Windows\\system32\\cmd.exe\x07hello \r\n"
        )
        == "hello\n"
    );
    TEST_ASSERT(drain_terminal_output_for_test(&filter).empty());
}

void test_terminal_output_filter_handles_split_escape_sequences() {
    TerminalOutputFilter filter;
    TEST_ASSERT(filter_terminal_output_for_test(&filter, "before\x1b[") == "before");
    TEST_ASSERT(filter_terminal_output_for_test(&filter, "0Kafter") == "after");
    TEST_ASSERT(drain_terminal_output_for_test(&filter).empty());
}

void test_terminal_output_filter_applies_backspace_to_utf8_codepoints() {
    TerminalOutputFilter filter;
    TEST_ASSERT(
        filter_terminal_output_for_test(&filter, "\xE4\xBD\xA0\xE5\xA5\xBD\x08!\r\n")
        == "\xE4\xBD\xA0!\n"
    );
}

void test_terminal_output_filter_collapses_winpty_prompt_rewrites() {
    TerminalOutputFilter filter;
    const std::string first = filter_terminal_output_for_test(
        &filter,
        "Microsoft Windows XP [\xE7\x89\x88\xE6\x9C\xAC 5.1.2600]"
        "\x1b[105G(C\r\n)\x20"
        "\xE7\x89\x88\xE6\x9D\x83\xE6\x89\x80\xE6\x9C\x89 1985-2001 Microsoft Corp.\r\n"
        "\x1b[107GC:\\chi\r\n>\r"
    );
    const std::string second = filter_terminal_output_for_test(
        &filter,
        "\x1b[107GC:\\chi>e\r\n"
        "cho hello\x1b[100Ghello\r\n"
        "\x1b[107GC:\\chi>\r"
    );
    const std::string final = first + second + drain_terminal_output_for_test(&filter);

    TEST_ASSERT(
        final.find("Microsoft Windows XP [\xE7\x89\x88\xE6\x9C\xAC 5.1.2600]") != std::string::npos
    );
    TEST_ASSERT(
        final.find("\xE7\x89\x88\xE6\x9D\x83\xE6\x89\x80\xE6\x9C\x89 1985-2001 Microsoft Corp.")
        != std::string::npos
    );
    TEST_ASSERT(final.find("C:\\chi>") != std::string::npos);
    TEST_ASSERT(final.find("hellohello") != std::string::npos);
    TEST_ASSERT(
        final.find("                                                                ")
        == std::string::npos
    );
    TEST_ASSERT(final.find('\x1b') == std::string::npos);
}

void test_terminal_output_filter_debounces_touched_rows() {
    TerminalOutputFilter filter(100UL, 500UL);
    TEST_ASSERT(filter_terminal_output_at_for_test(&filter, "hello", 1000U).empty());
    TEST_ASSERT(flush_terminal_output_due_for_test(&filter, 1099U).empty());
    TEST_ASSERT(flush_terminal_output_due_for_test(&filter, 1100U) == "hello");
    TEST_ASSERT(drain_terminal_output_for_test(&filter).empty());
}

void test_terminal_output_filter_debounce_uses_latest_repaint() {
    TerminalOutputFilter filter(100UL, 500UL);
    TEST_ASSERT(
        filter_terminal_output_at_for_test(&filter, "prompt>echo hello\x1b[116Ghell\r\no", 1000U)
            .empty()
    );
    TEST_ASSERT(flush_terminal_output_due_for_test(&filter, 1050U).empty());
    TEST_ASSERT(filter_terminal_output_at_for_test(
                    &filter,
                    "\r\x1b[1Aprompt>echo hello\x1b[0K\r\nhello",
                    1050U
    )
                    .empty());
    TEST_ASSERT(flush_terminal_output_due_for_test(&filter, 1149U).empty());

    const std::string emitted = flush_terminal_output_due_for_test(&filter, 1150U);
    TEST_ASSERT(emitted.find("prompt>echo hello") != std::string::npos);
    TEST_ASSERT(emitted.find("hello") != std::string::npos);
    TEST_ASSERT(emitted.find("hell\no") == std::string::npos);
}

void test_terminal_output_filter_debounce_drain_flushes_immediately() {
    TerminalOutputFilter filter(1000UL, 2000UL);
    TEST_ASSERT(filter_terminal_output_at_for_test(&filter, "prompt:", 1000U).empty());
    TEST_ASSERT(drain_terminal_output_for_test(&filter) == "prompt:");
}

void test_terminal_output_filter_does_not_finalize_partial_rows_on_flush_due() {
    if (should_skip_real_winpty_timing_tests()) {
        return;
    }

    TerminalOutputFilter filter(150UL, 500UL);
    TEST_ASSERT(
        filter_terminal_output_at_for_test(&filter, "prompt>echo hello\x1b[116Ghell\r\no", 1000U)
            .empty()
    );
    TEST_ASSERT(flush_terminal_output_due_for_test(&filter, 1100U).empty());
    TEST_ASSERT(filter_terminal_output_at_for_test(
                    &filter,
                    "\r\x1b[1Aprompt>echo hello\x1b[0K\r\nhello\r\n",
                    1150U
    )
                    .empty());
    TEST_ASSERT(flush_terminal_output_due_for_test(&filter, 1299U).empty());
    const std::string actual = flush_terminal_output_due_for_test(&filter, 1300U)
                               + drain_terminal_output_for_test(&filter);
    TEST_ASSERT(actual == "prompt>echo hello\nhello\n");
}

} // namespace

int main() {
    test_wide_to_utf8();
    test_utf8_to_wide();
    test_invalid_utf8_to_wide();
    test_code_page_decode();
    test_decode_carry();
    test_decode_carries_split_utf8_code_page();
    test_decode_carries_split_four_byte_code_page_sequence();
    test_decode_complete_dbcs_character_does_not_carry_trail_byte();
    test_invalid_decode_fallback();
    test_utf8_stream_decode_carry();
    test_utf8_stream_decode_flush_replaces_incomplete_suffix();
    test_utf8_stream_decode_replaces_invalid_scalar_sequences();
    test_terminal_output_filter_strips_winpty_control_sequences();
    test_terminal_output_filter_strips_osc_title_sequences();
    test_terminal_output_filter_handles_split_escape_sequences();
    test_terminal_output_filter_applies_backspace_to_utf8_codepoints();
    test_terminal_output_filter_collapses_winpty_prompt_rewrites();
    test_terminal_output_filter_debounces_touched_rows();
    test_terminal_output_filter_debounce_uses_latest_repaint();
    test_terminal_output_filter_debounce_drain_flushes_immediately();
    test_terminal_output_filter_does_not_finalize_partial_rows_on_flush_due();
    return 0;
}
