#include "test_assert.h"

#include <stdexcept>
#include <string>

#include <windows.h>

#include "exec/console_output.h"

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

void test_code_page_decode() {
    assert_code_page_decode(936U, "\xC4\xE3\xBA\xC3", "\xE4\xBD\xA0\xE5\xA5\xBD");
    assert_code_page_decode(932U, "\x82\xB1\x82\xF1\x82\xC9\x82\xBF\x82\xCD", "\xE3\x81\x93\xE3\x82\x93"
                                                                               "\xE3\x81\xAB\xE3\x81\xA1"
                                                                               "\xE3\x81\xAF");
    assert_code_page_decode(950U, "\xA7\x41\xA6\x6E", "\xE4\xBD\xA0\xE5\xA5\xBD");
}

void test_decode_carry() {
    if (!code_page_available(932U)) {
        return;
    }

    std::string carry;
    TEST_ASSERT(decode_console_output_for_test(932U, 1252U, &carry, "\x82", false).empty());
    TEST_ASSERT(carry == "\x82");
    TEST_ASSERT(decode_console_output_for_test(932U, 1252U, &carry, "\xB1", false) == "\xE3\x81\x93");
    TEST_ASSERT(carry.empty());
}

void test_invalid_decode_fallback() {
    std::string carry;
    TEST_ASSERT(decode_console_output_for_test(99999U, 99999U, &carry, "ok\xFF", true) == "ok\xEF\xBF\xBD");
    TEST_ASSERT(carry.empty());
}

} // namespace

int main() {
    test_wide_to_utf8();
    test_code_page_decode();
    test_decode_carry();
    test_invalid_decode_fallback();
    return 0;
}
