#pragma once

#ifndef _WIN32

#include <signal.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "platform/posix_eintr.h"

namespace posix_process {

inline pid_t fork_process() {
    return posix_eintr::retry<pid_t>([]() { return fork(); });
}

inline pid_t wait_pid(pid_t pid, int* status, int options) {
    return posix_eintr::retry<pid_t>([&]() { return waitpid(pid, status, options); });
}

inline int set_process_group(pid_t pid, pid_t pgid) {
    return posix_eintr::retry<int>([&]() { return setpgid(pid, pgid); });
}

inline int signal_process_group(pid_t pid, int signal_number) {
    return posix_eintr::retry<int>([&]() { return kill(-pid, signal_number); });
}

inline int execve_process(const char* path, char* const argv[], char* const envp[]) {
    return posix_eintr::retry<int>([&]() { return execve(path, argv, envp); });
}

} // namespace posix_process

#endif
