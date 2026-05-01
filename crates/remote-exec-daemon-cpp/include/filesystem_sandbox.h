#pragma once

#include <stdexcept>
#include <string>
#include <vector>

#include "path_policy.h"

struct SandboxPathList {
    std::vector<std::string> allow;
    std::vector<std::string> deny;
};

struct FilesystemSandbox {
    SandboxPathList exec_cwd;
    SandboxPathList read;
    SandboxPathList write;
};

enum SandboxAccess {
    SANDBOX_EXEC_CWD,
    SANDBOX_READ,
    SANDBOX_WRITE,
};

class SandboxError : public std::runtime_error {
public:
    explicit SandboxError(const std::string& message);
};

struct CompiledSandboxPathList {
    std::vector<std::string> allow;
    std::vector<std::string> deny;
};

struct CompiledFilesystemSandbox {
    CompiledSandboxPathList exec_cwd;
    CompiledSandboxPathList read;
    CompiledSandboxPathList write;
};

CompiledFilesystemSandbox compile_filesystem_sandbox(
    PathPolicy policy,
    const FilesystemSandbox& sandbox
);

void authorize_path(
    PathPolicy policy,
    const CompiledFilesystemSandbox* sandbox,
    SandboxAccess access,
    const std::string& path
);
