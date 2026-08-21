#ifndef _WIN32

#include "exec/locale.h"

#include <cerrno>
#include <cstdio>
#include <sstream>
#include <string>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "core/text_utils.h"

namespace {

bool is_utf8_locale(const std::string& locale) {
    const std::string lower = lowercase_ascii(locale);
    return (lower.size() >= 6U && lower.compare(lower.size() - 6U, 6U, ".utf-8") == 0)
           || (lower.size() >= 5U && lower.compare(lower.size() - 5U, 5U, ".utf8") == 0);
}

std::pair<unsigned char, std::string> locale_rank(const std::string& locale) {
    const std::string lower = lowercase_ascii(locale);
    if (lower == "en_us.utf-8")
        return std::make_pair(static_cast<unsigned char>(0), lower);
    if (lower == "en_us.utf8")
        return std::make_pair(static_cast<unsigned char>(1), lower);
    if (lower == "en_gb.utf-8")
        return std::make_pair(static_cast<unsigned char>(2), lower);
    if (lower == "en_gb.utf8")
        return std::make_pair(static_cast<unsigned char>(3), lower);
    if (lower.size() >= 3U && lower.compare(0, 3U, "en_") == 0 && is_utf8_locale(locale)) {
        return std::make_pair(static_cast<unsigned char>(4), lower);
    }
    return std::make_pair(static_cast<unsigned char>(5), lower);
}

std::vector<std::string> discover_locales() {
    std::vector<std::string> locales;
    int pipe_fds[2];
    if (pipe(pipe_fds) != 0)
        return locales;

    const pid_t pid = fork();
    if (pid < 0) {
        close(pipe_fds[0]);
        close(pipe_fds[1]);
        return locales;
    }
    if (pid == 0) {
        close(pipe_fds[0]);
        if (dup2(pipe_fds[1], STDOUT_FILENO) < 0)
            _exit(127);
        close(pipe_fds[1]);
        execlp("locale", "locale", "-a", static_cast<char*>(nullptr));
        _exit(127);
    }

    close(pipe_fds[1]);
    std::string output;
    char buffer[4096];
    ssize_t read_count;
    do {
        read_count = read(pipe_fds[0], buffer, sizeof(buffer));
        if (read_count > 0)
            output.append(buffer, static_cast<std::size_t>(read_count));
    } while (read_count > 0 || (read_count < 0 && errno == EINTR));
    close(pipe_fds[0]);

    int status = 0;
    pid_t waited;
    do {
        waited = waitpid(pid, &status, 0);
    } while (waited < 0 && errno == EINTR);
    if (status == -1 || !WIFEXITED(status) || WEXITSTATUS(status) != 0)
        locales.clear();
    if (waited != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0)
        return locales;

    std::istringstream lines(output);
    std::string line;
    while (std::getline(lines, line)) {
        const std::string locale = trim_ascii(line);
        if (!locale.empty())
            locales.push_back(locale);
    }
    return locales;
}

} // namespace

LocaleStrategy choose_locale_strategy(const std::vector<std::string>& input) {
    std::vector<std::string> locales;
    locales.reserve(input.size());
    for (std::size_t i = 0; i < input.size(); ++i) {
        const std::string locale = trim_ascii(input[i]);
        if (!locale.empty())
            locales.push_back(locale);
    }

    for (std::size_t i = 0; i < locales.size(); ++i) {
        if (locales[i] == "C.UTF-8")
            return LocaleStrategy{LocaleStrategyKind::Direct, "C.UTF-8"};
    }
    for (std::size_t i = 0; i < locales.size(); ++i) {
        if (locales[i] == "C.utf8")
            return LocaleStrategy{LocaleStrategyKind::Direct, "C.utf8"};
    }

    bool found = false;
    std::string best;
    std::pair<unsigned char, std::string> best_rank;
    for (std::size_t i = 0; i < locales.size(); ++i) {
        if (!is_utf8_locale(locales[i]))
            continue;
        const std::pair<unsigned char, std::string> rank = locale_rank(locales[i]);
        if (!found || rank < best_rank) {
            found = true;
            best = locales[i];
            best_rank = rank;
        }
    }
    if (found)
        return LocaleStrategy{LocaleStrategyKind::HybridCType, best};
    return LocaleStrategy{LocaleStrategyKind::LangCOnly, std::string()};
}

std::vector<std::pair<std::string, std::string>> LocaleEnvPlan::as_pairs() const {
    switch (strategy.kind) {
    case LocaleStrategyKind::Direct:
        return {
            std::make_pair("LANG", strategy.locale),
            std::make_pair("LC_CTYPE", strategy.locale),
            std::make_pair("LC_ALL", strategy.locale)
        };
    case LocaleStrategyKind::HybridCType:
        return {std::make_pair("LANG", "C"), std::make_pair("LC_CTYPE", strategy.locale)};
    case LocaleStrategyKind::LangCOnly:
        return {std::make_pair("LANG", "C")};
    }
    return {};
}

LocaleEnvPlan locale_env_plan_for_locales(const std::vector<std::string>& locales) {
    LocaleEnvPlan plan;
    plan.strategy = choose_locale_strategy(locales);
    return plan;
}

const LocaleEnvPlan& resolved_locale_env_plan() {
    static const LocaleEnvPlan plan = locale_env_plan_for_locales(discover_locales());
    return plan;
}

#endif
