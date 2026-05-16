#pragma once

#include <cstdio>
#include <string>
#include <sys/stat.h>

namespace path_utils {

char native_separator();
std::string parent_directory(const std::string& path);
std::string join_path(const std::string& base, const std::string& child);
void make_directory_if_missing(const std::string& path);
void create_parent_directories(const std::string& path);
FILE* open_file(const std::string& path, const char* mode);
bool stat_path(const std::string& path, struct stat* st);
bool lstat_path(const std::string& path, struct stat* st);
bool remove_path(const std::string& path);
bool remove_directory(const std::string& path);
bool rename_path(const std::string& source, const std::string& destination);

#ifdef _WIN32
std::wstring wide_from_utf8(const std::string& value);
std::string utf8_from_wide(const std::wstring& value);
#endif

} // namespace path_utils
