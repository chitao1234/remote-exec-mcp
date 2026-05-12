#pragma once

#include <functional>
#include <string>
#include <vector>

struct PatchApplyResult {
    std::string output;
    std::vector<std::string> updated_paths;
};

typedef std::function<void(const std::string&)> PatchPathAuthorizer;

PatchApplyResult apply_patch(const std::string& root,
                             const std::string& patch_text,
                             const PatchPathAuthorizer& authorizer = PatchPathAuthorizer());
