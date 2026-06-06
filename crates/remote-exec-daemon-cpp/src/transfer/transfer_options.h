#pragma once

#include <string>
#include <vector>

#include "transfer/transfer_ops.h"

namespace transfer_options {

struct ExportOptions {
    TransferSymlinkMode symlink_mode;
    std::vector<std::string> exclude;
};

void validate_transfer_options(const ExportOptions& options);

} // namespace transfer_options
