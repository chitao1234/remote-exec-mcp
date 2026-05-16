#include <algorithm>
#include <cctype>
#include <string>

#include "text_utils.h"

bool is_http_token_char(char ch) {
    const unsigned char value = static_cast<unsigned char>(ch);
    if (std::isalnum(value) != 0) {
        return true;
    }
    switch (ch) {
    case '!':
    case '#':
    case '$':
    case '%':
    case '&':
    case '\'':
    case '*':
    case '+':
    case '-':
    case '.':
    case '^':
    case '_':
    case '`':
    case '|':
    case '~':
        return true;
    default:
        return false;
    }
}

std::string trim_ascii(const std::string& raw) {
    const std::string whitespace = " \t\r\n";
    const std::size_t start = raw.find_first_not_of(whitespace);
    if (start == std::string::npos) {
        return "";
    }
    const std::size_t end = raw.find_last_not_of(whitespace);
    return raw.substr(start, end - start + 1);
}

std::string lowercase_ascii(std::string value) {
    std::transform(value.begin(), value.end(), value.begin(), [](unsigned char ch) {
        return static_cast<char>(std::tolower(ch));
    });
    return value;
}
