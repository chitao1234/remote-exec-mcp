#include <sstream>
#include <string>

#include <windows.h>

#include "win32_error.h"

std::string last_error_message(const char* prefix) {
    std::ostringstream out;
    out << prefix << " failed with error " << GetLastError();
    return out.str();
}
