#pragma once

#include <cstdio>

namespace test_progress {

inline void mark(const char* name) {
    std::fprintf(stderr, "[ test ] %s\n", name);
    std::fflush(stderr);
}

} // namespace test_progress
