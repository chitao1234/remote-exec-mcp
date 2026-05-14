#ifndef REMOTE_EXEC_DAEMON_CPP_TEST_TEXT_FILE_H
#define REMOTE_EXEC_DAEMON_CPP_TEST_TEXT_FILE_H

#include <string>

#include "test_filesystem.h"

inline void write_text_file(const test_fs::path& path, const std::string& contents) {
    test_fs::write_file_bytes(path, contents);
}

#endif
