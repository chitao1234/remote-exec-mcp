#pragma once

#ifndef _WIN32

#include <string>
#include <utility>
#include <vector>

enum class LocaleStrategyKind {
    Direct,
    HybridCType,
    LangCOnly,
};

struct LocaleStrategy {
    LocaleStrategyKind kind;
    std::string locale;
};

LocaleStrategy choose_locale_strategy(const std::vector<std::string>& locales);

struct LocaleEnvPlan {
    LocaleStrategy strategy;

    std::vector<std::pair<std::string, std::string>> as_pairs() const;
};

LocaleEnvPlan locale_env_plan_for_locales(const std::vector<std::string>& locales);
const LocaleEnvPlan& resolved_locale_env_plan();

#endif
