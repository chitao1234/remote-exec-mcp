#ifdef _WIN32

#include "platform/socket.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <stdexcept>
#include <string>
#include <vector>

#include "platform/win32_error.h"
#include "platform/win32_dynamic.h"
#include "platform/win32_socket_compat.h"
#include "platform/win32_winsock.h"

#include <windows.h>

namespace {

std::string socket_error_message_from_code(const std::string& operation, int error) {
    return error_message_from_code(operation.c_str(), static_cast<unsigned long>(error));
}

void throw_socket_option_error(const std::string& option, int error) {
    throw std::runtime_error(socket_error_message_from_code("setsockopt(" + option + ")", error));
}

int get_socket_name(SOCKET socket, sockaddr* address, socklen_t* address_len) {
#ifdef REMOTE_EXEC_CPP_WINSOCK1
    int len = static_cast<int>(*address_len);
    const int result = getsockname(socket, address, &len);
    *address_len = static_cast<socklen_t>(len);
    return result;
#else
    return getsockname(socket, address, address_len);
#endif
}

bool parse_numeric_port(const char* service, unsigned short* port) {
    if (service == nullptr || service[0] == '\0') {
        return false;
    }
    char* end = nullptr;
    const unsigned long parsed = std::strtoul(service, &end, 10);
    if (*end != '\0' || parsed > 65535UL) {
        return false;
    }
    *port = static_cast<unsigned short>(parsed);
    return true;
}

bool looks_like_ipv6_literal(const char* node) {
    return node != nullptr && std::strchr(node, ':') != nullptr;
}

bool append_ipv4_address(unsigned long ipv4,
                         unsigned short port,
                         int socktype,
                         int protocol,
                         std::vector<SocketAddress>* addresses) {
    SocketAddress address;
    address.family = AF_INET;
    address.socktype = socktype;
    address.protocol = protocol;
    sockaddr_in* ipv4_address = reinterpret_cast<sockaddr_in*>(&address.address);
    ipv4_address->sin_family = AF_INET;
    ipv4_address->sin_addr.s_addr = ipv4;
    ipv4_address->sin_port = htons(port);
    address.address_len = sizeof(sockaddr_in);
    addresses->push_back(address);
    return true;
}

std::string numeric_ipv4_socket_address(const sockaddr* address) {
    const sockaddr_in* ipv4 = reinterpret_cast<const sockaddr_in*>(address);
    const unsigned char* bytes = reinterpret_cast<const unsigned char*>(&ipv4->sin_addr.s_addr);
    char buffer[64];
    std::snprintf(buffer,
                  sizeof(buffer),
                  "%u.%u.%u.%u:%u",
                  static_cast<unsigned int>(bytes[0]),
                  static_cast<unsigned int>(bytes[1]),
                  static_cast<unsigned int>(bytes[2]),
                  static_cast<unsigned int>(bytes[3]),
                  static_cast<unsigned int>(ntohs(ipv4->sin_port)));
    return buffer;
}

bool resolve_legacy_ipv4_addresses(const char* backend_name,
                                   const char* node,
                                   unsigned short port,
                                   const SocketAddressQuery& query,
                                   std::vector<SocketAddress>* addresses,
                                   std::string* error) {
    if (query.family != AF_UNSPEC && query.family != AF_INET) {
        if (error != nullptr) {
            *error = std::string(backend_name) + " only supports IPv4 addresses";
        }
        return false;
    }

    if (looks_like_ipv6_literal(node)) {
        if (error != nullptr) {
            *error = std::string(backend_name) + " does not support IPv6 endpoints";
        }
        return false;
    }

    if (node == nullptr || node[0] == '\0') {
        const unsigned long passive_address = query.passive ? htonl(INADDR_ANY) : htonl(INADDR_LOOPBACK);
        return append_ipv4_address(passive_address, port, query.socktype, query.protocol, addresses);
    }

    const unsigned long numeric = inet_addr(node);
    if (numeric != INADDR_NONE || std::strcmp(node, "255.255.255.255") == 0) {
        return append_ipv4_address(numeric, port, query.socktype, query.protocol, addresses);
    }

    hostent* host = gethostbyname(node);
    if (host == nullptr) {
        if (error != nullptr) {
            *error = socket_error_message_from_code("gethostbyname", WSAGetLastError());
        }
        return false;
    }
    if (host->h_addrtype != AF_INET || host->h_length != static_cast<int>(sizeof(unsigned long))) {
        if (error != nullptr) {
            *error = std::string("gethostbyname failed: ") + backend_name + " only supports IPv4 addresses";
        }
        return false;
    }

    for (char** current = host->h_addr_list; current != nullptr && *current != nullptr; ++current) {
        unsigned long ipv4 = 0;
        std::memcpy(&ipv4, *current, sizeof(ipv4));
        append_ipv4_address(ipv4, port, query.socktype, query.protocol, addresses);
    }

    if (addresses->empty()) {
        if (error != nullptr) {
            *error = "gethostbyname failed: no usable IPv4 addresses";
        }
        return false;
    }
    return true;
}

#ifndef REMOTE_EXEC_CPP_WINSOCK1
typedef int(WSAAPI* GetAddrInfoFn)(const char*, const char*, const addrinfo*, addrinfo**);
typedef void(WSAAPI* FreeAddrInfoFn)(addrinfo*);
typedef int(WSAAPI* GetNameInfoFn)(const sockaddr*, socklen_t, char*, DWORD, char*, DWORD, int);

template <typename Fn>
Fn load_ws2_32_proc(const char* name) {
    HMODULE module = GetModuleHandleA("WS2_32.DLL");
    if (module == nullptr) {
        module = GetModuleHandleA("ws2_32.dll");
    }
    if (module == nullptr) {
        return nullptr;
    }
    return remote_exec_win32::proc_address_as<Fn>(GetProcAddress(module, name));
}

struct Winsock2AddressApi {
    GetAddrInfoFn getaddrinfo;
    FreeAddrInfoFn freeaddrinfo;
    GetNameInfoFn getnameinfo;
};

Winsock2AddressApi load_winsock2_address_api() {
    Winsock2AddressApi api;
    api.getaddrinfo = load_ws2_32_proc<GetAddrInfoFn>("getaddrinfo");
    api.freeaddrinfo = load_ws2_32_proc<FreeAddrInfoFn>("freeaddrinfo");
    api.getnameinfo = load_ws2_32_proc<GetNameInfoFn>("getnameinfo");
    return api;
}

bool resolve_winsock2_addresses(const char* node,
                                const char* service,
                                unsigned short port,
                                const SocketAddressQuery& query,
                                std::vector<SocketAddress>* addresses,
                                std::string* error) {
    const Winsock2AddressApi api = load_winsock2_address_api();
    if (api.getaddrinfo == nullptr || api.freeaddrinfo == nullptr) {
        return resolve_legacy_ipv4_addresses("Winsock 2 legacy resolver", node, port, query, addresses, error);
    }

    addrinfo hints;
    std::memset(&hints, 0, sizeof(hints));
    hints.ai_family = query.family;
    hints.ai_socktype = query.socktype;
    hints.ai_protocol = query.protocol;
    hints.ai_flags = query.passive ? AI_PASSIVE : 0;

    addrinfo* result = nullptr;
    const int status = api.getaddrinfo(node, service, &hints, &result);
    if (status != 0 || result == nullptr) {
        if (error != nullptr) {
            if (status == 0) {
                *error = "getaddrinfo failed";
            } else {
                *error = socket_error_message_from_code("getaddrinfo", status);
            }
        }
        return false;
    }

    for (addrinfo* current = result; current != nullptr; current = current->ai_next) {
        if (current->ai_addrlen > sizeof(SocketAddress().address)) {
            continue;
        }
        SocketAddress address;
        address.family = current->ai_family;
        address.socktype = current->ai_socktype;
        address.protocol = current->ai_protocol;
        address.address_len = static_cast<socklen_t>(current->ai_addrlen);
        std::memcpy(&address.address, current->ai_addr, current->ai_addrlen);
        addresses->push_back(address);
    }
    api.freeaddrinfo(result);

    if (addresses->empty()) {
        if (error != nullptr) {
            *error = "getaddrinfo failed: no usable socket addresses";
        }
        return false;
    }
    return true;
}
#endif

} // namespace

void close_socket(SOCKET socket) {
    closesocket(socket);
}

void shutdown_socket(SOCKET socket) {
    shutdown(socket, SD_BOTH);
}

void shutdown_socket_send(SOCKET socket) {
    shutdown(socket, SD_SEND);
}

bool set_socket_cloexec(SOCKET socket) {
    (void)socket;
    return true;
}

SOCKET create_socket_cloexec(int family, int type, int protocol) {
    return socket(family, type, protocol);
}

void set_socket_timeout_ms(SOCKET socket, unsigned long timeout_ms) {
    const DWORD value = static_cast<DWORD>(timeout_ms);
    if (setsockopt(socket, SOL_SOCKET, SO_RCVTIMEO, reinterpret_cast<const char*>(&value), sizeof(value)) != 0) {
        throw_socket_option_error("SO_RCVTIMEO", WSAGetLastError());
    }
    if (setsockopt(socket, SOL_SOCKET, SO_SNDTIMEO, reinterpret_cast<const char*>(&value), sizeof(value)) != 0) {
        throw_socket_option_error("SO_SNDTIMEO", WSAGetLastError());
    }
}

int last_socket_error() {
    return WSAGetLastError();
}

std::string socket_error_message(const std::string& operation) {
    return socket_error_message_from_code(operation, last_socket_error());
}

bool would_block_error(int error) {
    return error == WSAEWOULDBLOCK;
}

bool peer_disconnected_send_error(int error) {
    return error == WSAECONNABORTED || error == WSAECONNRESET || error == WSAESHUTDOWN;
}

bool receive_timeout_error(int error) {
    return error == WSAETIMEDOUT || error == WSAEWOULDBLOCK;
}

bool connect_in_progress_socket_error(int error) {
    return error == WSAEWOULDBLOCK || error == WSAEINPROGRESS;
}

NetworkSession::NetworkSession() {
    WSADATA wsa_data;
    if (remote_exec_win32::start_winsock(&wsa_data) != 0) {
        throw std::runtime_error("WSAStartup failed");
    }
}

NetworkSession::~NetworkSession() {
    WSACleanup();
}

bool resolve_socket_addresses(const char* node,
                              const char* service,
                              const SocketAddressQuery& query,
                              std::vector<SocketAddress>* addresses,
                              std::string* error) {
    addresses->clear();

    unsigned short port = 0;
    if (!parse_numeric_port(service, &port)) {
        if (error != nullptr) {
            *error = "invalid numeric port `" + std::string(service == nullptr ? "" : service) + "`";
        }
        return false;
    }

#ifdef REMOTE_EXEC_CPP_WINSOCK1
    return resolve_legacy_ipv4_addresses("Winsock 1 backend", node, port, query, addresses, error);
#else
    return resolve_winsock2_addresses(node, service, port, query, addresses, error);
#endif
}

std::string numeric_socket_address(const sockaddr* address, socklen_t address_len) {
    (void)address_len;
#ifdef REMOTE_EXEC_CPP_WINSOCK1
    if (address->sa_family != AF_INET) {
        return "unknown:0";
    }
    return numeric_ipv4_socket_address(address);
#else
    const Winsock2AddressApi api = load_winsock2_address_api();
    if (api.getnameinfo == nullptr) {
        if (address->sa_family == AF_INET) {
            return numeric_ipv4_socket_address(address);
        }
        return "unknown:0";
    }

    char host[NI_MAXHOST];
    char service[NI_MAXSERV];
    const int result = api.getnameinfo(address,
                                       address_len,
                                       host,
                                       static_cast<DWORD>(sizeof(host)),
                                       service,
                                       static_cast<DWORD>(sizeof(service)),
                                       NI_NUMERICHOST | NI_NUMERICSERV);
    if (result != 0) {
        return "unknown:0";
    }

    if (address->sa_family == AF_INET6) {
        return "[" + std::string(host) + "]:" + std::string(service);
    }
    return std::string(host) + ":" + std::string(service);
#endif
}

int wait_socket_readable_or_wakeup(SOCKET socket, SOCKET wakeup_fd, unsigned long timeout_ms) {
    fd_set readfds;
    FD_ZERO(&readfds);
    FD_SET(socket, &readfds);
    FD_SET(wakeup_fd, &readfds);

    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    const int ready = select(0, &readfds, nullptr, nullptr, &timeout);
    if (ready <= 0) {
        return ready;
    }
    if (FD_ISSET(wakeup_fd, &readfds)) {
        return -1;
    }
    return ready;
}

int wait_socket_readable(SOCKET socket, unsigned long timeout_ms) {
    fd_set readfds;
    FD_ZERO(&readfds);
    FD_SET(socket, &readfds);

    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    return select(0, &readfds, nullptr, nullptr, &timeout);
}

int wait_socket_writable(SOCKET socket, unsigned long timeout_ms) {
    fd_set writefds;
    FD_ZERO(&writefds);
    FD_SET(socket, &writefds);

    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    const int selected = select(0, nullptr, &writefds, nullptr, &timeout);
    if (selected <= 0) {
        return selected;
    }
    return FD_ISSET(socket, &writefds) ? 1 : 0;
}

int socket_error_option(SOCKET socket, int* socket_error) {
    socklen_t socket_error_len = static_cast<socklen_t>(sizeof(*socket_error));
#ifdef REMOTE_EXEC_CPP_WINSOCK1
    int socket_error_len_int = static_cast<int>(socket_error_len);
    return getsockopt(socket, SOL_SOCKET, SO_ERROR, reinterpret_cast<char*>(socket_error), &socket_error_len_int);
#else
    return getsockopt(socket, SOL_SOCKET, SO_ERROR, reinterpret_cast<char*>(socket_error), &socket_error_len);
#endif
}

int set_socket_reuseaddr(SOCKET socket) {
    int yes = 1;
    return setsockopt(socket, SOL_SOCKET, SO_REUSEADDR, reinterpret_cast<const char*>(&yes), sizeof(yes));
}

int set_socket_ipv6_only(SOCKET socket) {
#ifdef REMOTE_EXEC_CPP_WINSOCK1
    (void)socket;
    WSASetLastError(WSAEAFNOSUPPORT);
    return -1;
#else
    int yes = 1;
    return setsockopt(socket, IPPROTO_IPV6, IPV6_V6ONLY, reinterpret_cast<const char*>(&yes), sizeof(yes));
#endif
}

int bind_socket(SOCKET socket, const sockaddr* address, socklen_t address_len) {
    return bind(socket, address, static_cast<int>(address_len));
}

int listen_socket(SOCKET socket, int backlog) {
    return listen(socket, backlog);
}

int socket_name(SOCKET socket, sockaddr* address, socklen_t* address_len) {
    return get_socket_name(socket, address, address_len);
}

unsigned short socket_bound_port_or_zero(SOCKET socket) {
    if (socket == INVALID_SOCKET) {
        return 0;
    }

    sockaddr_storage address;
    std::memset(&address, 0, sizeof(address));
    socklen_t address_len = sizeof(address);
    if (get_socket_name(socket, reinterpret_cast<sockaddr*>(&address), &address_len) != 0) {
        return 0;
    }

    if (address.ss_family == AF_INET) {
        const sockaddr_in* ipv4 = reinterpret_cast<const sockaddr_in*>(&address);
        return ntohs(ipv4->sin_port);
    }
#ifndef REMOTE_EXEC_CPP_WINSOCK1
    if (address.ss_family == AF_INET6) {
        const sockaddr_in6* ipv6 = reinterpret_cast<const sockaddr_in6*>(&address);
        return ntohs(ipv6->sin6_port);
    }
#endif

    return 0;
}

#endif
