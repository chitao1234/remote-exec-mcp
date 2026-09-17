#pragma once

#include <string>

#include "core/legacy_encoding.h"

// Statistical encoding detection, backed by the vendored Mozilla universal
// charset detector (third_party/uchardet).
//
// Detection only reports what the bytes most likely are. It never converts
// anything: the caller decodes through core/legacy_encoding and verifies the
// result round-trips before touching the file.
//
// The whole feature is compiled out when REMOTE_EXEC_CPP_HAS_TEXT_ENCODING is
// undefined. Builds without it keep the historical UTF-8-only behavior and do
// not link the detector or the conversion tables at all.

#ifdef REMOTE_EXEC_CPP_HAS_TEXT_ENCODING

// Returns true when the bytes were confidently identified as a legacy encoding
// the daemon can convert. ASCII-only content is reported as UTF-8, which the
// caller treats as the identity encoding.
bool detect_legacy_encoding(const std::string& bytes, LegacyEncoding* out);

// The detector's raw label for the bytes (for example "SHIFT_JIS"), or an empty
// string when nothing was detected. Kept separate from
// detect_legacy_encoding so diagnostics can name whatever the detector chose,
// including encodings the daemon does not implement.
std::string detect_encoding_label(const std::string& bytes);

#else

inline bool detect_legacy_encoding(const std::string&, LegacyEncoding*) {
    return false;
}

inline std::string detect_encoding_label(const std::string&) {
    return std::string();
}

#endif
