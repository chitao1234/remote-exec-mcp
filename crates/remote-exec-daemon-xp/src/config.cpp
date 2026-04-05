#include <cstdlib>
#include <fstream>
#include <map>
#include <stdexcept>

#include "config.h"
#include "text_utils.h"

static std::string unquote(const std::string& raw) {
    if (raw.size() >= 2 && raw.front() == '"' && raw.back() == '"') {
        return raw.substr(1, raw.size() - 2);
    }
    return raw;
}

DaemonConfig load_config(const std::string& path) {
    std::ifstream input(path.c_str());
    if (!input) {
        throw std::runtime_error("unable to open config file: " + path);
    }

    std::map<std::string, std::string> values;
    std::string line;
    while (std::getline(input, line)) {
        line = trim_ascii(line);
        if (line.empty() || line[0] == '#' || line[0] == ';') {
            continue;
        }

        const std::size_t equals = line.find('=');
        if (equals == std::string::npos) {
            throw std::runtime_error("invalid config line: " + line);
        }

        const std::string key = trim_ascii(line.substr(0, equals));
        const std::string value = unquote(trim_ascii(line.substr(equals + 1)));
        values[key] = value;
    }

    DaemonConfig config;
    config.target = values.at("target");
    config.listen_host = values.at("listen_host");
    config.listen_port = std::atoi(values.at("listen_port").c_str());
    config.default_workdir = values.at("default_workdir");
    return config;
}
