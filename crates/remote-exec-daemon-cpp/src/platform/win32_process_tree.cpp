#ifdef _WIN32

#include "platform/win32_process_tree.h"

#include <map>
#include <string>
#include <vector>

#include <tlhelp32.h>

#include "core/logging.h"
#include "platform/win32_dynamic.h"
#include "platform/win32_error.h"
#include "platform/win32_scoped.h"

namespace {

typedef HANDLE(WINAPI* CreateToolhelp32SnapshotFn)(DWORD, DWORD);
typedef BOOL(WINAPI* Process32FirstWFn)(HANDLE, LPPROCESSENTRY32W);
typedef BOOL(WINAPI* Process32NextWFn)(HANDLE, LPPROCESSENTRY32W);

struct ToolhelpApi {
    ToolhelpApi()
        : create_snapshot(nullptr), process_first(nullptr), process_next(nullptr), loaded(false) {}

    CreateToolhelp32SnapshotFn create_snapshot;
    Process32FirstWFn process_first;
    Process32NextWFn process_next;
    bool loaded;
};

ToolhelpApi load_toolhelp_api() {
    ToolhelpApi api;
    HMODULE kernel32 = GetModuleHandleW(L"kernel32.dll");
    if (kernel32 == nullptr) {
        return api;
    }

    api.create_snapshot = remote_exec_win32::proc_address_as<CreateToolhelp32SnapshotFn>(
        GetProcAddress(kernel32, "CreateToolhelp32Snapshot")
    );
    api.process_first = remote_exec_win32::proc_address_as<Process32FirstWFn>(
        GetProcAddress(kernel32, "Process32FirstW")
    );
    api.process_next = remote_exec_win32::proc_address_as<Process32NextWFn>(
        GetProcAddress(kernel32, "Process32NextW")
    );
    api.loaded = api.create_snapshot != nullptr && api.process_first != nullptr
                 && api.process_next != nullptr;
    return api;
}

const ToolhelpApi& toolhelp_api() {
    static const ToolhelpApi api = load_toolhelp_api();
    return api;
}

struct ProcessEntry {
    ProcessEntry() : pid(0U), parent_pid(0U) {}

    DWORD pid;
    DWORD parent_pid;
};

std::vector<ProcessEntry> snapshot_processes(const ToolhelpApi& api, bool* supported) {
    std::vector<ProcessEntry> entries;
    *supported = false;

    if (!api.loaded) {
        log_message(LOG_WARN, "process_tree", "Toolhelp process snapshot APIs are unavailable");
        return entries;
    }

    UniqueHandle snapshot(api.create_snapshot(TH32CS_SNAPPROCESS, 0U));
    if (!snapshot.valid()) {
        log_message(
            LOG_WARN,
            "process_tree",
            std::string("CreateToolhelp32Snapshot failed: ")
                + last_error_message("CreateToolhelp32Snapshot")
        );
        return entries;
    }

    PROCESSENTRY32W raw_entry;
    ZeroMemory(&raw_entry, sizeof(raw_entry));
    raw_entry.dwSize = sizeof(raw_entry);

    if (api.process_first(snapshot.get(), &raw_entry) == 0) {
        const DWORD error = GetLastError();
        if (error != ERROR_NO_MORE_FILES) {
            log_message(
                LOG_WARN,
                "process_tree",
                std::string("Process32FirstW failed: ")
                    + error_message_from_code("Process32FirstW", error)
            );
        }
        return entries;
    }

    *supported = true;
    for (;;) {
        ProcessEntry entry;
        entry.pid = raw_entry.th32ProcessID;
        entry.parent_pid = raw_entry.th32ParentProcessID;
        entries.push_back(entry);

        raw_entry.dwSize = sizeof(raw_entry);
        if (api.process_next(snapshot.get(), &raw_entry) == 0) {
            const DWORD error = GetLastError();
            if (error != ERROR_NO_MORE_FILES) {
                log_message(
                    LOG_WARN,
                    "process_tree",
                    std::string("Process32NextW failed: ")
                        + error_message_from_code("Process32NextW", error)
                );
            }
            break;
        }
    }

    return entries;
}

void append_descendants(
    DWORD pid,
    const std::map<DWORD, std::vector<DWORD>>& children_by_parent,
    std::vector<DWORD>* process_ids
) {
    std::map<DWORD, std::vector<DWORD>>::const_iterator children = children_by_parent.find(pid);
    if (children == children_by_parent.end()) {
        return;
    }

    for (std::size_t i = 0; i < children->second.size(); ++i) {
        const DWORD child_pid = children->second[i];
        append_descendants(child_pid, children_by_parent, process_ids);
        process_ids->push_back(child_pid);
    }
}

bool terminate_pid(DWORD pid) {
    UniqueHandle process(OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, FALSE, pid));
    if (!process.valid()) {
        const DWORD error = GetLastError();
        if (error == ERROR_INVALID_PARAMETER) {
            return true;
        }
        log_message(
            LOG_WARN,
            "process_tree",
            std::string("OpenProcess failed for pid ") + std::to_string(pid) + ": "
                + error_message_from_code("OpenProcess", error)
        );
        return false;
    }

    if (TerminateProcess(process.get(), 1U) == 0) {
        const DWORD error = GetLastError();
        log_message(
            LOG_WARN,
            "process_tree",
            std::string("TerminateProcess failed for pid ") + std::to_string(pid) + ": "
                + error_message_from_code("TerminateProcess", error)
        );
        return false;
    }
    return true;
}

} // namespace

namespace win32_process_tree {

namespace {

bool terminate_process_tree_impl(DWORD root_pid, bool include_root) {
    if (root_pid == 0U) {
        return false;
    }

    bool supported = false;
    const std::vector<ProcessEntry> entries = snapshot_processes(toolhelp_api(), &supported);
    if (!supported) {
        return false;
    }

    std::map<DWORD, std::vector<DWORD>> children_by_parent;
    for (std::size_t i = 0; i < entries.size(); ++i) {
        if (entries[i].parent_pid != 0U && entries[i].pid != 0U
            && entries[i].pid != entries[i].parent_pid) {
            children_by_parent[entries[i].parent_pid].push_back(entries[i].pid);
        }
    }

    std::vector<DWORD> process_ids;
    append_descendants(root_pid, children_by_parent, &process_ids);
    if (include_root) {
        process_ids.push_back(root_pid);
    }

    bool attempted = false;
    for (std::size_t i = 0; i < process_ids.size(); ++i) {
        attempted = terminate_pid(process_ids[i]) || attempted;
    }
    return attempted;
}

} // namespace

bool process_tree_snapshot_supported() {
    return toolhelp_api().loaded;
}

bool terminate_process_descendants(DWORD root_pid) {
    return terminate_process_tree_impl(root_pid, false);
}

bool terminate_process_tree(DWORD root_pid) {
    return terminate_process_tree_impl(root_pid, true);
}

} // namespace win32_process_tree

#endif
