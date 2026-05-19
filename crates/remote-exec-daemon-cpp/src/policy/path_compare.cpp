#include <cctype>
#include <stdexcept>
#include <string>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#endif

#include "path_compare.h"
#include "path_policy.h"

namespace {

struct ParsedPath {
    std::string prefix;
    std::vector<std::string> components;
};

bool is_ascii_alpha(char ch) {
    return std::isalpha(static_cast<unsigned char>(ch)) != 0;
}

bool is_separator(PathStyle style, char ch) {
    if (style == PATH_STYLE_WINDOWS) {
        return ch == '\\' || ch == '/';
    }
    return ch == '/';
}

ParsedPath parse_host_path(const std::string& raw) {
    const PathPolicy policy = host_path_policy();
    const std::string normalized = normalize_for_system(policy, raw);
    ParsedPath parsed;
    std::size_t start = 0;

    if (policy.style == PATH_STYLE_POSIX) {
        if (!normalized.empty() && normalized[0] == '/') {
            parsed.prefix = "/";
            start = 1;
        }
    } else if (normalized.size() >= 3 && is_ascii_alpha(normalized[0]) && normalized[1] == ':' &&
               is_separator(policy.style, normalized[2])) {
        parsed.prefix = normalized.substr(0, 2);
        parsed.prefix.push_back('\\');
        start = 3;
    } else if (normalized.rfind("\\\\", 0) == 0) {
        parsed.prefix = "\\\\";
        start = 2;
    }

    std::string current;
    for (std::size_t i = start; i < normalized.size(); ++i) {
        const char ch = normalized[i];
        if (is_separator(policy.style, ch)) {
            if (!current.empty()) {
                parsed.components.push_back(current);
                current.clear();
            }
            continue;
        }
        current.push_back(ch);
    }

    if (!current.empty()) {
        parsed.components.push_back(current);
    }

    return parsed;
}

#ifdef _WIN32
typedef int(WINAPI * CompareStringOrdinalFn)(const WCHAR*, int, const WCHAR*, int, BOOL);

std::wstring wide_from_utf8(const std::string& value) {
    if (value.empty()) {
        return std::wstring();
    }

    const int wide_length =
        MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), nullptr, 0);
    if (wide_length <= 0) {
        throw std::runtime_error("unable to decode UTF-8 path");
    }

    std::wstring wide(static_cast<std::size_t>(wide_length), L'\0');
    if (MultiByteToWideChar(
            CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), &wide[0], wide_length) <=
        0) {
        throw std::runtime_error("unable to decode UTF-8 path");
    }
    return wide;
}

CompareStringOrdinalFn compare_string_ordinal_fn() {
    static CompareStringOrdinalFn fn = nullptr;
    static bool initialized = false;
    if (!initialized) {
        HMODULE kernel32 = GetModuleHandleW(L"kernel32.dll");
        if (kernel32 != nullptr) {
            fn = reinterpret_cast<CompareStringOrdinalFn>(GetProcAddress(kernel32, "CompareStringOrdinal"));
        }
        initialized = true;
    }
    return fn;
}

bool ascii_case_equal(const std::string& left, const std::string& right) {
    if (left.size() != right.size()) {
        return false;
    }

    for (std::size_t i = 0; i < left.size(); ++i) {
        const unsigned char left_ch = static_cast<unsigned char>(left[i]);
        const unsigned char right_ch = static_cast<unsigned char>(right[i]);
        if (std::tolower(left_ch) != std::tolower(right_ch)) {
            return false;
        }
    }

    return true;
}

bool component_equal(const std::string& left, const std::string& right) {
    try {
        const std::wstring left_wide = wide_from_utf8(left);
        const std::wstring right_wide = wide_from_utf8(right);
        const CompareStringOrdinalFn ordinal = compare_string_ordinal_fn();
        if (ordinal != nullptr) {
            return ordinal(
                       left_wide.empty() ? nullptr : left_wide.data(),
                       static_cast<int>(left_wide.size()),
                       right_wide.empty() ? nullptr : right_wide.data(),
                       static_cast<int>(right_wide.size()),
                       TRUE) == CSTR_EQUAL;
        }
        return CompareStringW(
                   LOCALE_INVARIANT,
                   NORM_IGNORECASE,
                   left_wide.empty() ? nullptr : left_wide.data(),
                   static_cast<int>(left_wide.size()),
                   right_wide.empty() ? nullptr : right_wide.data(),
                   static_cast<int>(right_wide.size())) == CSTR_EQUAL;
    } catch (const std::exception&) {
        return ascii_case_equal(left, right);
    }
}
#else
bool component_equal(const std::string& left, const std::string& right) {
    return left == right;
}
#endif

bool prefixes_equal(const ParsedPath& left, const ParsedPath& right) {
    return component_equal(left.prefix, right.prefix);
}

} // namespace

bool host_path_equal(const std::string& left, const std::string& right) {
    const ParsedPath left_path = parse_host_path(left);
    const ParsedPath right_path = parse_host_path(right);
    if (!prefixes_equal(left_path, right_path)) {
        return false;
    }
    if (left_path.components.size() != right_path.components.size()) {
        return false;
    }
    for (std::size_t i = 0; i < left_path.components.size(); ++i) {
        if (!component_equal(left_path.components[i], right_path.components[i])) {
            return false;
        }
    }
    return true;
}

bool host_path_has_prefix(const std::string& path, const std::string& prefix) {
    const ParsedPath path_value = parse_host_path(path);
    const ParsedPath prefix_value = parse_host_path(prefix);
    if (!prefixes_equal(path_value, prefix_value)) {
        return false;
    }
    if (path_value.components.size() < prefix_value.components.size()) {
        return false;
    }
    for (std::size_t i = 0; i < prefix_value.components.size(); ++i) {
        if (!component_equal(path_value.components[i], prefix_value.components[i])) {
            return false;
        }
    }
    return true;
}

bool host_path_is_within(const std::string& path, const std::string& root) {
    return host_path_has_prefix(path, root);
}
