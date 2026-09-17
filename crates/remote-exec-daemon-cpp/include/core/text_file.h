#pragma once

#include <cstdint>
#include <stdexcept>
#include <string>

// Text decoding for the file tools and the patch engine.
//
// The FastPath is byte-identical to UTF-8, which is what legacy daemon
// behavior always assumed. When decoding with autodetection enabled, a file
// that is not valid UTF-8 is decoded with a detected encoding and rewritten
// in that same encoding, so a read/modify/write cycle preserves the bytes
// around the change.
//
// Detected encodings other than the UTF family are deliberately excluded from
// the UnicodeDetector stage until the conversion tables land; see
// detect_and_decode below.

struct DetectedTextEncoding {
    DetectedTextEncoding() : kind_id(0U), bom_length(0U) {}

    // Stable identifier for the detected encoding. Only the decoder in this
    // translation unit interprets it.
    std::uint32_t kind_id;
    // Leading bytes reproduced verbatim when the text is written back.
    std::string bom;
    // Length of the byte-order mark removed from the original content.
    std::size_t bom_length;
};

class TextFileError : public std::runtime_error {
public:
    enum Kind {
        READ_FAILED,
        DECODE_FAILED,
        ENCODE_FAILED,
        BINARY,
    };

    TextFileError(Kind kind, const std::string& message)
        : std::runtime_error(message), kind_(kind) {}

    Kind kind() const { return kind_; }

private:
    Kind kind_;
};

// Decoded text plus the encoding needed to write it back.
class TextFile {
public:
    // Decode raw file bytes. `allow_autodetect` mirrors the Rust daemon's
    // experimental_apply_patch_target_encoding_autodetect switch; when false
    // only UTF-8 is accepted.
    static TextFile from_bytes(const std::string& bytes, bool allow_autodetect);

    // Encode `text` back into the original encoding. Fails when a character
    // cannot be represented, which is what the Rust daemon does too.
    std::string encode(const std::string& text) const;

    const std::string& text() const { return text_; }

private:
    std::string text_;
    DetectedTextEncoding encoding_;
};

// True when decoded text looks like text rather than a binary blob: no NUL
// bytes, and only a small share of control characters.
bool looks_like_text(const std::string& text);
