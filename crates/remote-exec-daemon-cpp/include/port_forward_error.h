#pragma once

#include <stdexcept>
#include <string>

class PortForwardError : public std::runtime_error {
public:
    PortForwardError(int status, const std::string& code, const std::string& message);

    int status() const;
    const std::string& code() const;

private:
    int status_;
    std::string code_;
};
