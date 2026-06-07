#pragma once

#include <vector>

#include "exec/session_store.h"
#include "http/http_helpers.h"

Json exec_session_warnings_json(const std::vector<ExecSessionWarning>& warnings);
Json exec_session_result_json(const ExecSessionResult& result, unsigned long max_output_tokens);
