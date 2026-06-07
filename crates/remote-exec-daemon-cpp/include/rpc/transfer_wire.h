#pragma once

#include <string>

#include "transfer/transfer_types.h"

const char* transfer_source_type_wire_value(TransferSourceType source_type);
bool parse_transfer_source_type_wire_value(
    const std::string& value,
    TransferSourceType* source_type
);
const char* transfer_symlink_mode_wire_value(TransferSymlinkMode symlink_mode);
bool parse_transfer_symlink_mode_wire_value(
    const std::string& value,
    TransferSymlinkMode* symlink_mode
);
const char* transfer_overwrite_wire_value(TransferOverwrite overwrite);
bool parse_transfer_overwrite_wire_value(const std::string& value, TransferOverwrite* overwrite);
