#include <sstream>
#include <string>

#include "platform/win32_socket_compat.h"

#include <windows.h>

#include "platform/win32_error.h"

namespace {

const char* error_symbol(unsigned long error) {
    switch (error) {
    case ERROR_SUCCESS:
        return "ERROR_SUCCESS";
    case ERROR_FILE_NOT_FOUND:
        return "ERROR_FILE_NOT_FOUND";
    case ERROR_PATH_NOT_FOUND:
        return "ERROR_PATH_NOT_FOUND";
    case ERROR_ACCESS_DENIED:
        return "ERROR_ACCESS_DENIED";
    case ERROR_INVALID_HANDLE:
        return "ERROR_INVALID_HANDLE";
    case ERROR_NOT_ENOUGH_MEMORY:
        return "ERROR_NOT_ENOUGH_MEMORY";
    case ERROR_INVALID_DATA:
        return "ERROR_INVALID_DATA";
    case ERROR_INVALID_PARAMETER:
        return "ERROR_INVALID_PARAMETER";
    case ERROR_BROKEN_PIPE:
        return "ERROR_BROKEN_PIPE";
    case ERROR_SEM_TIMEOUT:
        return "ERROR_SEM_TIMEOUT";
    case ERROR_OPERATION_ABORTED:
        return "ERROR_OPERATION_ABORTED";
    case ERROR_ALREADY_EXISTS:
        return "ERROR_ALREADY_EXISTS";
    case ERROR_FILE_EXISTS:
        return "ERROR_FILE_EXISTS";
    case ERROR_SHARING_VIOLATION:
        return "ERROR_SHARING_VIOLATION";
    case ERROR_LOCK_VIOLATION:
        return "ERROR_LOCK_VIOLATION";
    case ERROR_DIRECTORY:
        return "ERROR_DIRECTORY";
    case ERROR_INVALID_NAME:
        return "ERROR_INVALID_NAME";
    case ERROR_BAD_PATHNAME:
        return "ERROR_BAD_PATHNAME";
    case ERROR_DIR_NOT_EMPTY:
        return "ERROR_DIR_NOT_EMPTY";
    case ERROR_HANDLE_EOF:
        return "ERROR_HANDLE_EOF";
    case ERROR_PIPE_BUSY:
        return "ERROR_PIPE_BUSY";
    case ERROR_NO_DATA:
        return "ERROR_NO_DATA";
    case ERROR_PIPE_NOT_CONNECTED:
        return "ERROR_PIPE_NOT_CONNECTED";
    case ERROR_IO_PENDING:
        return "ERROR_IO_PENDING";
    case ERROR_NETNAME_DELETED:
        return "ERROR_NETNAME_DELETED";
    case WSAEINTR:
        return "WSAEINTR";
    case WSAEBADF:
        return "WSAEBADF";
    case WSAEACCES:
        return "WSAEACCES";
    case WSAEFAULT:
        return "WSAEFAULT";
    case WSAEINVAL:
        return "WSAEINVAL";
    case WSAEMFILE:
        return "WSAEMFILE";
    case WSAEWOULDBLOCK:
        return "WSAEWOULDBLOCK";
    case WSAEINPROGRESS:
        return "WSAEINPROGRESS";
    case WSAEALREADY:
        return "WSAEALREADY";
    case WSAENOTSOCK:
        return "WSAENOTSOCK";
    case WSAEDESTADDRREQ:
        return "WSAEDESTADDRREQ";
    case WSAEMSGSIZE:
        return "WSAEMSGSIZE";
    case WSAEPROTOTYPE:
        return "WSAEPROTOTYPE";
    case WSAENOPROTOOPT:
        return "WSAENOPROTOOPT";
    case WSAEPROTONOSUPPORT:
        return "WSAEPROTONOSUPPORT";
    case WSAESOCKTNOSUPPORT:
        return "WSAESOCKTNOSUPPORT";
    case WSAEOPNOTSUPP:
        return "WSAEOPNOTSUPP";
    case WSAEPFNOSUPPORT:
        return "WSAEPFNOSUPPORT";
    case WSAEAFNOSUPPORT:
        return "WSAEAFNOSUPPORT";
    case WSAEADDRINUSE:
        return "WSAEADDRINUSE";
    case WSAEADDRNOTAVAIL:
        return "WSAEADDRNOTAVAIL";
    case WSAENETDOWN:
        return "WSAENETDOWN";
    case WSAENETUNREACH:
        return "WSAENETUNREACH";
    case WSAENETRESET:
        return "WSAENETRESET";
    case WSAECONNABORTED:
        return "WSAECONNABORTED";
    case WSAECONNRESET:
        return "WSAECONNRESET";
    case WSAENOBUFS:
        return "WSAENOBUFS";
    case WSAEISCONN:
        return "WSAEISCONN";
    case WSAENOTCONN:
        return "WSAENOTCONN";
    case WSAESHUTDOWN:
        return "WSAESHUTDOWN";
    case WSAETIMEDOUT:
        return "WSAETIMEDOUT";
    case WSAECONNREFUSED:
        return "WSAECONNREFUSED";
    case WSAEHOSTDOWN:
        return "WSAEHOSTDOWN";
    case WSAEHOSTUNREACH:
        return "WSAEHOSTUNREACH";
    case WSAHOST_NOT_FOUND:
        return "WSAHOST_NOT_FOUND";
    case WSATRY_AGAIN:
        return "WSATRY_AGAIN";
    case WSANO_RECOVERY:
        return "WSANO_RECOVERY";
    case WSANO_DATA:
        return "WSANO_DATA";
    default:
        return nullptr;
    }
}

} // namespace

std::string error_message_from_code(const char* prefix, unsigned long error) {
    std::ostringstream out;
    out << prefix << " failed";
    const char* symbol = error_symbol(error);
    if (symbol != nullptr) {
        out << ": " << symbol << " (error " << error << ")";
    } else {
        out << " with error " << error;
    }
    return out.str();
}

std::string last_error_message(const char* prefix) {
    return error_message_from_code(prefix, GetLastError());
}
