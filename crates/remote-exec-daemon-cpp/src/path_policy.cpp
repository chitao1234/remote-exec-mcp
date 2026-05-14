#include <algorithm>
#include <cctype>
#include <cwctype>
#include <locale>
#include <stdexcept>
#include <string>

#ifdef _WIN32
#include <windows.h>
#else
#include <codecvt>
#endif

#include "path_policy.h"

namespace {

bool is_ascii_alpha(char ch) {
    return std::isalpha(static_cast<unsigned char>(ch)) != 0;
}

std::string normalize_windows_path_chars(std::string value) {
    std::replace(value.begin(), value.end(), '/', '\\');
    return value;
}

std::string build_windows_drive_path(char drive, const std::string& rest) {
    std::string tail = rest;
    while (!tail.empty() && (tail[0] == '/' || tail[0] == '\\')) {
        tail.erase(0, 1);
    }

    std::string output;
    output.push_back(static_cast<char>(std::toupper(static_cast<unsigned char>(drive))));
    output += ":\\";
    if (!tail.empty()) {
        output += normalize_windows_path_chars(tail);
    }
    return output;
}

bool translate_windows_posix_drive_path(const std::string& raw, std::string* output) {
    if (raw.size() >= 2 && raw[0] == '/' && is_ascii_alpha(raw[1]) && (raw.size() == 2 || raw[2] == '/')) {
        *output = build_windows_drive_path(raw[1], raw.substr(raw.size() == 2 ? 2 : 3));
        return true;
    }

    const std::string prefix = "/cygdrive/";
    const std::string lower = path_policy_lowercase_ascii(raw);
    if (lower.rfind(prefix, 0) != 0) {
        return false;
    }
    if (raw.size() <= prefix.size() || !is_ascii_alpha(raw[prefix.size()])) {
        return false;
    }
    if (raw.size() > prefix.size() + 1 && raw[prefix.size() + 1] != '/') {
        return false;
    }

    *output = build_windows_drive_path(
        raw[prefix.size()], raw.substr(raw.size() == prefix.size() + 1 ? prefix.size() + 1 : prefix.size() + 2));
    return true;
}

std::string lowercase_utf8_windows_path(const std::string& value);

} // namespace

PathPolicy posix_path_policy() {
    PathPolicy policy;
    policy.style = PATH_STYLE_POSIX;
    policy.comparison = PATH_COMPARISON_CASE_SENSITIVE;
    return policy;
}

PathPolicy windows_path_policy() {
    PathPolicy policy;
    policy.style = PATH_STYLE_WINDOWS;
    policy.comparison = PATH_COMPARISON_CASE_INSENSITIVE;
    return policy;
}

PathPolicy host_path_policy() {
#ifdef _WIN32
    return windows_path_policy();
#else
    return posix_path_policy();
#endif
}

std::string path_policy_lowercase_ascii(std::string value) {
    for (std::size_t i = 0; i < value.size(); ++i) {
        value[i] = static_cast<char>(std::tolower(static_cast<unsigned char>(value[i])));
    }
    return value;
}

#ifdef _WIN32
namespace {

std::wstring wide_from_utf8(const std::string& value) {
    if (value.empty()) {
        return std::wstring();
    }

    const int wide_length =
        MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), NULL, 0);
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

std::string utf8_from_wide(const std::wstring& value) {
    if (value.empty()) {
        return std::string();
    }

    const int utf8_length =
        WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), NULL, 0, NULL, NULL);
    if (utf8_length <= 0) {
        throw std::runtime_error("unable to encode UTF-8 path");
    }

    std::string utf8(static_cast<std::size_t>(utf8_length), '\0');
    if (WideCharToMultiByte(
            CP_UTF8, 0, value.data(), static_cast<int>(value.size()), &utf8[0], utf8_length, NULL, NULL) <= 0) {
        throw std::runtime_error("unable to encode UTF-8 path");
    }
    return utf8;
}
#else
namespace {

std::wstring wide_from_utf8(const std::string& value) {
    std::wstring_convert<std::codecvt_utf8_utf16<wchar_t> > converter;
    return converter.from_bytes(value);
}

std::string utf8_from_wide(const std::wstring& value) {
    std::wstring_convert<std::codecvt_utf8_utf16<wchar_t> > converter;
    return converter.to_bytes(value);
}
#endif

std::string lowercase_utf8_windows_path(const std::string& value) {
    try {
        std::wstring wide = wide_from_utf8(value);
        if (wide.empty()) {
            return std::string();
        }

#ifdef _WIN32
        CharLowerBuffW(&wide[0], static_cast<DWORD>(wide.size()));
#else
        // POSIX hosts only use this path in tests that exercise Windows semantics.
        std::locale locale("");
        const std::ctype<wchar_t>& ctype = std::use_facet<std::ctype<wchar_t> >(locale);
        ctype.tolower(&wide[0], &wide[0] + wide.size());
#endif
        return utf8_from_wide(wide);
    } catch (const std::exception&) {
        return path_policy_lowercase_ascii(value);
    }
}

} // namespace

std::string path_policy_comparison_key(PathPolicy policy, const std::string& raw) {
    std::string normalized = normalize_for_system(policy, raw);
    if (policy.comparison == PATH_COMPARISON_CASE_INSENSITIVE) {
        normalized = lowercase_utf8_windows_path(normalized);
    }
    return normalized;
}

bool is_absolute_for_policy(PathPolicy policy, const std::string& raw) {
    if (policy.style == PATH_STYLE_POSIX) {
        return !raw.empty() && raw[0] == '/';
    }

    if (raw.size() >= 3 && is_ascii_alpha(raw[0]) && raw[1] == ':' && (raw[2] == '\\' || raw[2] == '/')) {
        return true;
    }
    if (raw.rfind("\\\\", 0) == 0 || raw.rfind("//", 0) == 0) {
        return true;
    }

    std::string translated;
    return translate_windows_posix_drive_path(raw, &translated);
}

std::string normalize_for_system(PathPolicy policy, const std::string& raw) {
    if (policy.style == PATH_STYLE_POSIX) {
        return raw;
    }

    std::string translated;
    if (translate_windows_posix_drive_path(raw, &translated)) {
        return translated;
    }
    return normalize_windows_path_chars(raw);
}

std::string join_for_policy(PathPolicy policy, const std::string& base, const std::string& child) {
    const std::string normalized_child = normalize_for_system(policy, child);
    if (normalized_child.empty() || is_absolute_for_policy(policy, normalized_child)) {
        return normalized_child;
    }

    const std::string normalized_base = normalize_for_system(policy, base);
    if (normalized_base.empty()) {
        return normalized_child;
    }

    const char separator = policy.style == PATH_STYLE_WINDOWS ? '\\' : '/';
    if (normalized_base[normalized_base.size() - 1] == separator) {
        return normalized_base + normalized_child;
    }
    return normalized_base + separator + normalized_child;
}

bool same_path_for_policy(PathPolicy policy, const std::string& left, const std::string& right) {
    return path_policy_comparison_key(policy, left) == path_policy_comparison_key(policy, right);
}
