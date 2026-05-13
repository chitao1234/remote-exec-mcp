#ifndef REMOTE_EXEC_DAEMON_CPP_TEST_TEXT_FILE_H
#define REMOTE_EXEC_DAEMON_CPP_TEST_TEXT_FILE_H

#include <cassert>
#include <fstream>
#include <string>

#include "test_filesystem.h"

inline void write_text_file(const test_fs::path& path, const std::string& contents) {
    std::ofstream output(path.string().c_str(), std::ios::binary | std::ios::trunc);
    assert(output.good());
    output << contents;
}

#endif
