#pragma once

#include <cstdio>
#include <cstdlib>

namespace test_assert {

inline void fail(const char* expression, const char* file, int line) {
    std::fprintf(stderr, "%s:%d: test assertion failed: %s\n", file, line, expression);
    std::fflush(stderr);
    std::abort();
}

inline void require(bool condition, const char* expression, const char* file, int line) {
    if (!condition) {
        fail(expression, file, line);
    }
}

}  // namespace test_assert

#define TEST_ASSERT(...)                                                                          \
    do {                                                                                          \
        ::test_assert::require(static_cast<bool>((__VA_ARGS__)), #__VA_ARGS__, __FILE__,         \
                                __LINE__);                                                        \
    } while (0)
