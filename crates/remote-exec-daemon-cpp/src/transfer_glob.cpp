#include "transfer_glob.h"

#include <cstddef>
#include <stdexcept>
#include <string>
#include <vector>

#include "rpc_failures.h"

namespace {

TransferFailure invalid_pattern(
    const std::string& pattern,
    const std::string& reason
) {
    return TransferFailure(
        TransferRpcCode::TransferFailed,
        "invalid exclude pattern `" + pattern + "`: " + reason
    );
}

std::size_t character_class_end(
    const std::string& pattern,
    std::size_t open_index
) {
    std::size_t index = open_index + 1U;
    if (index < pattern.size() &&
        (pattern[index] == '!' || pattern[index] == '^')) {
        ++index;
    }
    if (index >= pattern.size()) {
        throw invalid_pattern(pattern, "unterminated character class");
    }

    bool has_member = false;
    for (; index < pattern.size(); ++index) {
        if (pattern[index] == ']' && has_member) {
            return index;
        }
        has_member = true;
    }
    throw invalid_pattern(pattern, "unterminated character class");
}

void validate_pattern(const std::string& pattern) {
    for (std::size_t index = 0; index < pattern.size(); ++index) {
        if (pattern[index] != '[') {
            continue;
        }
        index = character_class_end(pattern, index);
    }
}

bool matches_character_class(
    const std::string& pattern,
    std::size_t open_index,
    char candidate,
    std::size_t* next_index
) {
    const std::size_t close_index = character_class_end(pattern, open_index);
    std::size_t index = open_index + 1U;
    bool negated = false;
    if (pattern[index] == '!' || pattern[index] == '^') {
        negated = true;
        ++index;
    }

    bool matched = false;
    while (index < close_index) {
        const char start = pattern[index];
        if (index + 2U < close_index && pattern[index + 1U] == '-') {
            const char end = pattern[index + 2U];
            if (candidate >= start && candidate <= end) {
                matched = true;
            }
            index += 3U;
            continue;
        }
        if (candidate == start) {
            matched = true;
        }
        ++index;
    }

    *next_index = close_index + 1U;
    return negated ? !matched : matched;
}

bool match_pattern(
    const std::string& pattern,
    std::size_t pattern_index,
    const std::string& text,
    std::size_t text_index
) {
    while (true) {
        if (pattern_index == pattern.size()) {
            return text_index == text.size();
        }
        if (pattern[pattern_index] == '*') {
            if (pattern_index + 1U < pattern.size() &&
                pattern[pattern_index + 1U] == '*') {
                const std::size_t next_index = pattern_index + 2U;
                if (next_index < pattern.size() && pattern[next_index] == '/') {
                    if (match_pattern(pattern, next_index + 1U, text, text_index)) {
                        return true;
                    }
                    for (std::size_t index = text_index; index < text.size(); ++index) {
                        if (text[index] == '/' &&
                            match_pattern(pattern, next_index + 1U, text, index + 1U)) {
                            return true;
                        }
                    }
                    return false;
                }
                if (match_pattern(pattern, next_index, text, text_index)) {
                    return true;
                }
                for (std::size_t index = text_index; index < text.size(); ++index) {
                    if (match_pattern(pattern, next_index, text, index + 1U)) {
                        return true;
                    }
                }
                return false;
            }

            if (match_pattern(pattern, pattern_index + 1U, text, text_index)) {
                return true;
            }
            for (std::size_t index = text_index;
                 index < text.size() && text[index] != '/';
                 ++index) {
                if (match_pattern(pattern, pattern_index + 1U, text, index + 1U)) {
                    return true;
                }
            }
            return false;
        }
        if (pattern[pattern_index] == '?') {
            if (text_index == text.size() || text[text_index] == '/') {
                return false;
            }
            ++pattern_index;
            ++text_index;
            continue;
        }
        if (pattern[pattern_index] == '[') {
            if (text_index == text.size() || text[text_index] == '/') {
                return false;
            }
            std::size_t next_index = pattern_index;
            if (!matches_character_class(
                    pattern,
                    pattern_index,
                    text[text_index],
                    &next_index)) {
                return false;
            }
            pattern_index = next_index;
            ++text_index;
            continue;
        }
        if (text_index == text.size() ||
            pattern[pattern_index] != text[text_index]) {
            return false;
        }
        ++pattern_index;
        ++text_index;
    }
}

}  // namespace

namespace transfer_glob {

Matcher::Matcher() {}

Matcher::Matcher(const std::vector<std::string>& patterns) : patterns_(patterns) {
    for (std::size_t index = 0; index < patterns_.size(); ++index) {
        validate_pattern(patterns_[index]);
    }
}

bool Matcher::is_excluded_path(const std::string& relative_path) const {
    if (relative_path.empty()) {
        return false;
    }
    for (std::size_t index = 0; index < patterns_.size(); ++index) {
        if (match_pattern(patterns_[index], 0U, relative_path, 0U)) {
            return true;
        }
    }
    return false;
}

bool Matcher::is_excluded_directory(const std::string& relative_path) const {
    if (is_excluded_path(relative_path)) {
        return true;
    }
    if (relative_path.empty()) {
        return false;
    }
    return is_excluded_path(relative_path + "/");
}

}  // namespace transfer_glob
