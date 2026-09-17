#include "core/encoding_detect.h"

#ifdef REMOTE_EXEC_CPP_HAS_TEXT_ENCODING

#include <cstddef>
#include <string>

#include "uchardet/src/uchardet.h"

namespace {

const std::size_t MAX_DETECTION_BYTES = 1024U * 1024U;

// The detector's window. Feeding a bounded prefix keeps detection cost flat
// for large files while still covering far more text than the detector's own
// sampling window uses.
void feed_detector(uchardet_t detector, const std::string& bytes) {
    const std::size_t limit =
        bytes.size() < MAX_DETECTION_BYTES ? bytes.size() : MAX_DETECTION_BYTES;
    if (limit > 0U) {
        uchardet_handle_data(detector, bytes.data(), limit);
    }
    uchardet_data_end(detector);
}

} // namespace

std::string detect_encoding_label(const std::string& bytes) {
    if (bytes.empty()) {
        return "";
    }

    uchardet_t detector = uchardet_new();
    if (detector == NULL) {
        return "";
    }
    feed_detector(detector, bytes);
    const char* charset = uchardet_get_charset(detector);
    const std::string label = charset == NULL ? std::string() : std::string(charset);
    uchardet_delete(detector);
    return label;
}

bool detect_legacy_encoding(const std::string& bytes, LegacyEncoding* out) {
    out->id = LEGACY_ENCODING_NONE;
    out->single_byte_index = 0U;

    const std::string label = detect_encoding_label(bytes);
    if (label.empty()) {
        return false;
    }

    // ASCII and UTF-8 need no conversion, and the caller has already tried the
    // UTF-8 fast path, so neither is a legacy encoding here.
    if (label == "ASCII" || label == "UTF-8") {
        return false;
    }

    return legacy_encoding_from_label(label, out);
}

#endif
