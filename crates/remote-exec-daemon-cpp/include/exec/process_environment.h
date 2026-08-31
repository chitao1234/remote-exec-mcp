#pragma once

#include <string>
#include <utility>
#include <vector>

typedef std::pair<std::string, std::string> ProcessEnvironmentPair;

std::vector<ProcessEnvironmentPair> normalized_process_environment_pairs(
    const std::vector<ProcessEnvironmentPair>& locale_pairs
);
