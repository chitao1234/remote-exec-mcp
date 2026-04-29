#pragma once

#include <string>

struct PatchApplyResult {
    std::string output;
};

PatchApplyResult apply_patch(const std::string& root, const std::string& patch_text);
