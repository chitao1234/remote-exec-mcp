#pragma once

#include <string>
#include <vector>

#include "http_helpers.h"
#include "logging.h"
#include "server.h"
#include "transfer_ops.h"

void require_uncompressed_transfer(const std::string& compression);
std::string transfer_symlink_mode_or_default(const std::string& symlink_mode);
Json transfer_warnings_json(const std::vector<TransferWarning>& warnings);
Json transfer_summary_json(const ImportSummary& summary);
void log_transfer_import_summary(const std::string& destination_path, const ImportSummary& summary);
LogLevel level_for_status(int status);
HttpResponse route_request(AppState& state, const HttpRequest& request);
