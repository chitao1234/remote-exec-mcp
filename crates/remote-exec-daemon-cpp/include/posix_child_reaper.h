#pragma once

#ifndef _WIN32
#include <sys/types.h>

void install_posix_child_reaper();
void register_posix_child(pid_t pid);
void unregister_posix_child(pid_t pid);
bool take_reaped_posix_child(pid_t pid, int* status);
bool poll_posix_child_exit(pid_t pid, int* status);
bool wait_posix_child_exit(pid_t pid, int* status);

#ifdef REMOTE_EXEC_CPP_TESTING
void set_posix_child_reaper_test_reap_delay_ms(unsigned long delay_ms);
#endif

#endif
