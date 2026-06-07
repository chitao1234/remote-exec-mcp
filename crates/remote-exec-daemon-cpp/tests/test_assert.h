#pragma once

#include <cstdio>
#include <cstdlib>
#include <string>

namespace test_assert {

inline void fail(const char* expression, const char* file, int line) {
    std::fprintf(stderr, "%s:%d: test assertion failed: %s\n", file, line, expression);
    std::fflush(stderr);
    std::abort();
}

inline void fail_message(const std::string& message, const char* file, int line) {
    std::fprintf(stderr, "%s:%d: test assertion failed: %s\n", file, line, message.c_str());
    std::fflush(stderr);
    std::abort();
}

inline void require(bool condition, const char* expression, const char* file, int line) {
    if (!condition) {
        fail(expression, file, line);
    }
}

} // namespace test_assert

#define TEST_ASSERT(...)                                                                                               \
    do {                                                                                                               \
        ::test_assert::require(static_cast<bool>((__VA_ARGS__)), #__VA_ARGS__, __FILE__, __LINE__);                    \
    } while (0)

#define TEST_FAIL_MESSAGE(message)                                                                                     \
    do {                                                                                                               \
        ::test_assert::fail_message((message), __FILE__, __LINE__);                                                    \
    } while (0)
