#include "core/legacy_encoding.h"

#ifndef REMOTE_EXEC_CPP_HAS_TEXT_ENCODING
// Without the text encoding feature the daemon only handles UTF-8, so no
// conversion is available and the label lookup always fails.
bool legacy_encoding_from_label(const std::string&, LegacyEncoding* out) {
    out->id = LEGACY_ENCODING_NONE;
    out->single_byte_index = 0U;
    return false;
}

const char* legacy_encoding_name(const LegacyEncoding&) {
    return "unknown";
}

bool legacy_encoding_decode(const LegacyEncoding&, const std::string&, std::string*) {
    return false;
}

bool legacy_encoding_encode(const LegacyEncoding&, const std::string&, std::string*) {
    return false;
}

#endif
