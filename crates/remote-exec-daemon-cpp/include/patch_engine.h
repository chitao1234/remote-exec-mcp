#pragma once

#include <functional>
#include <string>

struct PatchApplyResult {
    std::string output;
};

typedef std::function<void(const std::string&)> PatchPathAuthorizer;

PatchApplyResult apply_patch(
    const std::string& root,
    const std::string& patch_text,
    const PatchPathAuthorizer& authorizer = PatchPathAuthorizer()
);
