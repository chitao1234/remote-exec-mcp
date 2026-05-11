#pragma once

#ifndef _WIN32
#include <sys/types.h>

void install_posix_child_reaper();
void register_posix_child(pid_t pid);
void unregister_posix_child(pid_t pid);
bool take_reaped_posix_child(pid_t pid, int* status);

#endif
