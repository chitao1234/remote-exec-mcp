#include "test_assert.h"

#include <string>

#include "platform/socket.h"
#include "port_forward/port_forward_error.h"
#include "port_forward/port_forward_socket_ops.h"

namespace {

void assert_ipv6_endpoint_rejected() {
    NetworkSession network;

    bool rejected = false;
    try {
        UniqueSocket socket(bind_port_forward_socket("[::1]:0", "tcp"));
        (void)socket;
    } catch (const PortForwardError& ex) {
        rejected = true;
        TEST_ASSERT(ex.status() == 400);
        TEST_ASSERT(ex.code() == "invalid_endpoint");
        TEST_ASSERT(std::string(ex.what()).find("IPv6 endpoint") != std::string::npos);
        TEST_ASSERT(std::string(ex.what()).find("Winsock 1") != std::string::npos);
    }
    TEST_ASSERT(rejected);
}

} // namespace

int main() {
    assert_ipv6_endpoint_rejected();
    return 0;
}
