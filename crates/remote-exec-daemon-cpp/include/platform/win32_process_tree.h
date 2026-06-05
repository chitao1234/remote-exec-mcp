#pragma once

#ifdef _WIN32

#include <windows.h>

namespace win32_process_tree {

bool process_tree_snapshot_supported();
bool terminate_process_descendants(DWORD root_pid);
bool terminate_process_tree(DWORD root_pid);

} // namespace win32_process_tree

#endif
