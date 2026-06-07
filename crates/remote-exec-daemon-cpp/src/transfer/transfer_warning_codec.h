#pragma once

#include <string>
#include <vector>

#include "transfer/transfer_types.h"

namespace transfer_warning_codec {

std::string transfer_summary_body(const std::vector<TransferWarning>& warnings);
std::vector<TransferWarning> read_transfer_summary(const std::string& body);
void append_warnings(std::vector<TransferWarning>* destination, const std::vector<TransferWarning>& source);

} // namespace transfer_warning_codec
