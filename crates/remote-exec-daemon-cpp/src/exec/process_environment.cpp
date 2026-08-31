#include "exec/process_environment.h"

std::vector<ProcessEnvironmentPair> normalized_process_environment_pairs(
    const std::vector<ProcessEnvironmentPair>& locale_pairs
) {
    std::vector<ProcessEnvironmentPair> pairs;
    pairs.reserve(7U + locale_pairs.size());
    pairs.push_back(std::make_pair("NO_COLOR", "1"));
    pairs.push_back(std::make_pair("TERM", "dumb"));
    pairs.push_back(std::make_pair("COLORTERM", ""));
    pairs.push_back(std::make_pair("PAGER", "cat"));
    pairs.push_back(std::make_pair("GIT_PAGER", "cat"));
    pairs.push_back(std::make_pair("GH_PAGER", "cat"));
    pairs.push_back(std::make_pair("CODEX_CI", "1"));
    pairs.insert(pairs.end(), locale_pairs.begin(), locale_pairs.end());
    return pairs;
}
