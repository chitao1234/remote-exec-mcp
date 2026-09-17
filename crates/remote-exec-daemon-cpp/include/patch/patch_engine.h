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

// Options that change how patch targets are read and written.
struct PatchApplyOptions {
    PatchApplyOptions() : allow_encoding_autodetect(false) {}

    // Decode an existing target with its detected encoding and write the
    // result back in that encoding. Requires a build with the text encoding
    // feature; without it this flag has no effect.
    bool allow_encoding_autodetect;
};

PatchApplyResult apply_patch(
    const std::string& root,
    const std::string& patch_text,
    const PatchPathAuthorizer& authorizer = PatchPathAuthorizer(),
    const std::string& windows_posix_root = std::string()
);

PatchApplyResult apply_patch_with_options(
    const std::string& root,
    const std::string& patch_text,
    const PatchPathAuthorizer& authorizer,
    const std::string& windows_posix_root,
    const PatchApplyOptions& options
);
