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
}

void test_invalid_decode_fallback() {
    std::string carry;
    TEST_ASSERT(decode_console_output_for_test(99999U, 99999U, &carry, "ok\xFF", true) == "ok\xEF\xBF\xBD");
    TEST_ASSERT(carry.empty());
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
    return 0;
}
