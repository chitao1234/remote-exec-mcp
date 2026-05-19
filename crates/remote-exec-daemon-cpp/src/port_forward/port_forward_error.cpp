#include "port_forward_error.h"

PortForwardError::PortForwardError(int status, const std::string& code, const std::string& message)
    : std::runtime_error(message), status_(status), code_(code) {
}

int PortForwardError::status() const {
    return status_;
}

const std::string& PortForwardError::code() const {
    return code_;
}
