#pragma once

#include <cstdint>
#include <string>

// Legacy encoding conversion driven by the WHATWG Encoding Standard index
// tables.
//
// Each encoding has a two-way API: decode a byte string into UTF-8, and encode
// UTF-8 back into the original bytes. Encoding fails rather than substituting
// when a character has no representation, because the daemon must never
// silently rewrite bytes the caller did not intend to change.

// Stable identifiers for the encodings this module supports. The numeric
// values are persisted only in memory; they are not a wire format.
enum LegacyEncodingId {
    LEGACY_ENCODING_NONE = 0,
    LEGACY_ENCODING_SHIFT_JIS,
    LEGACY_ENCODING_EUC_JP,
    LEGACY_ENCODING_GB18030,
    LEGACY_ENCODING_BIG5,
    LEGACY_ENCODING_EUC_KR,
    // A single-byte encoding, identified by its table slot.
    LEGACY_ENCODING_SINGLE_BYTE_BASE,
};

struct LegacyEncoding {
    LegacyEncoding() : id(LEGACY_ENCODING_NONE), single_byte_index(0U) {}

    std::uint32_t id;
    std::uint32_t single_byte_index;
};

// Resolves a detector label such as "SHIFT_JIS" or "WINDOWS-1251" to an
// encoding. Returns false for labels the daemon does not implement.
bool legacy_encoding_from_label(const std::string& label, LegacyEncoding* out);

// Human-readable name for diagnostics and round-trip error messages.
const char* legacy_encoding_name(const LegacyEncoding& encoding);

// Decodes into UTF-8. Returns false when the bytes are not valid for the
// encoding, including any unmapped byte sequence.
bool legacy_encoding_decode(
    const LegacyEncoding& encoding,
    const std::string& bytes,
    std::string* utf8_out
);

// Encodes UTF-8 back into the encoding. Returns false when a code point has no
// representation in the encoding.
bool legacy_encoding_encode(
    const LegacyEncoding& encoding,
    const std::string& utf8,
    std::string* bytes_out
);
