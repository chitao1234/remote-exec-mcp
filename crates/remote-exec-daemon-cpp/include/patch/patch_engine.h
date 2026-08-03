#pragma once

#include <functional>
#include <stdexcept>
#include <string>
#include <vector>

struct PatchApplyResult {
    std::string output;
    std::vector<std::string> updated_paths;
    std::string environment_id;
};

typedef std::function<void(const std::string&)> PatchPathAuthorizer;

class PatchApplyPartialError : public std::runtime_error {
public:
    PatchApplyPartialError(
        const std::string& message,
        const std::vector<std::string>& updated_paths
    )
        : std::runtime_error(message), updated_paths_(updated_paths) {}

    const std::vector<std::string>& updated_paths() const { return updated_paths_; }

private:
    std::vector<std::string> updated_paths_;
};

PatchApplyResult apply_patch(
    const std::string& root,
    const std::string& patch_text,
    const PatchPathAuthorizer& authorizer = PatchPathAuthorizer()
);
