#include <algorithm>
#include <cctype>
#include <string>

#include "text_utils.h"

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
    std::transform(
        value.begin(),
        value.end(),
        value.begin(),
        [](unsigned char ch) { return static_cast<char>(std::tolower(ch)); }
    );
    return value;
}
