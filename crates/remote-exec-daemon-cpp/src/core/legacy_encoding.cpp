#include "core/legacy_encoding.h"

#ifdef REMOTE_EXEC_CPP_HAS_TEXT_ENCODING

#include <cstddef>
#include <cstring>
#include <string>

#include "whatwg-encoding/encoding_tables.h"

namespace {

namespace wt = whatwg_tables;

const std::uint32_t REPLACEMENT_LIMIT = 0x10FFFFU;

void append_utf8(std::uint32_t code_point, std::string* out) {
    if (code_point <= 0x7FU) {
        out->push_back(static_cast<char>(code_point));
        return;
    }
    if (code_point <= 0x7FFU) {
        out->push_back(static_cast<char>(0xC0U | (code_point >> 6)));
        out->push_back(static_cast<char>(0x80U | (code_point & 0x3FU)));
        return;
    }
    if (code_point <= 0xFFFFU) {
        out->push_back(static_cast<char>(0xE0U | (code_point >> 12)));
        out->push_back(static_cast<char>(0x80U | ((code_point >> 6) & 0x3FU)));
        out->push_back(static_cast<char>(0x80U | (code_point & 0x3FU)));
        return;
    }
    out->push_back(static_cast<char>(0xF0U | (code_point >> 18)));
    out->push_back(static_cast<char>(0x80U | ((code_point >> 12) & 0x3FU)));
    out->push_back(static_cast<char>(0x80U | ((code_point >> 6) & 0x3FU)));
    out->push_back(static_cast<char>(0x80U | (code_point & 0x3FU)));
}

bool single_byte_index_for_label(const std::string& label, std::uint32_t* out) {
    for (std::size_t i = 0;
         i < sizeof(wt::SINGLE_BYTE_ALIASES) / sizeof(wt::SINGLE_BYTE_ALIASES[0]);
         ++i) {
        if (label == wt::SINGLE_BYTE_ALIASES[i].label) {
            *out = wt::SINGLE_BYTE_ALIASES[i].table_index;
            return true;
        }
    }

    // The canonical table names are accepted directly as well, so a config
    // value or detector report does not need to go through an alias.
    static const char* const CANONICAL[] = {
        "WINDOWS-1250", "WINDOWS-1251", "WINDOWS-1252", "WINDOWS-1253", "WINDOWS-1254",
        "WINDOWS-1255", "WINDOWS-1256", "WINDOWS-1257", "WINDOWS-1258", "WINDOWS-874",
        "ISO-8859-2",   "ISO-8859-3",   "ISO-8859-4",   "ISO-8859-5",   "ISO-8859-6",
        "ISO-8859-7",   "ISO-8859-8",   "ISO-8859-10",  "ISO-8859-13",  "ISO-8859-14",
        "ISO-8859-15",  "ISO-8859-16",  "KOI8-R",       "KOI8-U",       "MACINTOSH",
        "IBM866",
    };
    const std::size_t canonical_count = sizeof(CANONICAL) / sizeof(CANONICAL[0]);
    if (canonical_count != wt::SINGLE_BYTE_ENCODING_COUNT) {
        return false;
    }
    for (std::size_t i = 0; i < canonical_count; ++i) {
        if (label == CANONICAL[i]) {
            *out = static_cast<std::uint32_t>(i);
            return true;
        }
    }
    return false;
}

std::uint16_t decode_single_byte(const LegacyEncoding& encoding, unsigned char byte) {
    if (byte < 0x80U) {
        return static_cast<std::uint16_t>(byte);
    }
    return wt::SINGLE_BYTE_TABLE[encoding.single_byte_index][byte - 0x80U];
}

// Binary search for a pointer given a code point, over the generated
// parallel arrays.
bool lookup_pointer(
    const std::uint32_t* points,
    const std::uint16_t* pointers,
    std::size_t count,
    std::uint32_t code_point,
    std::uint32_t* out
) {
    std::size_t low = 0U;
    std::size_t high = count;
    while (low < high) {
        const std::size_t middle = low + (high - low) / 2U;
        if (points[middle] < code_point) {
            low = middle + 1U;
        } else {
            high = middle;
        }
    }
    if (low < count && points[low] == code_point) {
        *out = pointers[low];
        return true;
    }
    return false;
}

// Decodes an ASCII byte the way every multi-byte decoder in the spec does.
bool decode_ascii(unsigned char byte, std::string* out) {
    if (byte >= 0x80U) {
        return false;
    }
    out->push_back(static_cast<char>(byte));
    return true;
}

bool decode_shift_jis(const std::string& bytes, std::string* out) {
    for (std::size_t i = 0; i < bytes.size(); ++i) {
        const unsigned char byte = static_cast<unsigned char>(bytes[i]);
        if (decode_ascii(byte, out)) {
            continue;
        }
        if (byte >= 0xA1U && byte <= 0xDFU) {
            append_utf8(0xFF61U - 0xA1U + byte, out);
            continue;
        }
        if (byte == 0x80U || (byte >= 0xA0U && byte <= 0xA0U)) {
            // 0x80 passes through unchanged; 0xA0 is unmapped.
            if (byte == 0x80U) {
                append_utf8(0x80U, out);
                continue;
            }
            return false;
        }

        if (i + 1U >= bytes.size()) {
            return false;
        }
        const unsigned char trail = static_cast<unsigned char>(bytes[i + 1U]);
        bool valid_trail = false;
        if (trail >= 0x40U && trail <= 0x7EU) {
            valid_trail = true;
        } else if (trail >= 0x80U && trail <= 0xFCU && trail != 0x7FU) {
            valid_trail = true;
        }
        if (!valid_trail) {
            return false;
        }
        const std::uint32_t lead_offset = byte < 0xA0U ? 0x81U : 0xC1U;
        const std::uint32_t trail_offset = trail < 0x7FU ? 0x40U : 0x41U;
        const std::uint32_t pointer = (byte - lead_offset) * 188U + (trail - trail_offset);
        if (pointer >= 8836U && pointer <= 10715U) {
            append_utf8(0xE000U - 8836U + pointer, out);
            i += 1U;
            continue;
        }
        if (pointer >= 11280U) {
            return false;
        }
        const std::uint16_t point = wt::INDEX_JIS0208[pointer];
        if (point == 0U) {
            return false;
        }
        append_utf8(point, out);
        i += 1U;
    }
    return true;
}

bool decode_euc_jp(const std::string& bytes, std::string* out) {
    for (std::size_t i = 0; i < bytes.size(); ++i) {
        const unsigned char byte = static_cast<unsigned char>(bytes[i]);
        if (decode_ascii(byte, out)) {
            continue;
        }
        if (byte == 0x8EU) {
            if (i + 1U >= bytes.size()) {
                return false;
            }
            const unsigned char trail = static_cast<unsigned char>(bytes[i + 1U]);
            if (trail < 0xA1U || trail > 0xDFU) {
                return false;
            }
            append_utf8(0xFF61U - 0xA1U + trail, out);
            i += 1U;
            continue;
        }
        if (byte == 0x8FU) {
            if (i + 2U >= bytes.size()) {
                return false;
            }
            const unsigned char lead = static_cast<unsigned char>(bytes[i + 1U]);
            const unsigned char trail = static_cast<unsigned char>(bytes[i + 2U]);
            if (lead < 0xA1U || lead > 0xFEU || trail < 0xA1U || trail > 0xFEU) {
                return false;
            }
            const std::uint32_t pointer = (lead - 0xA1U) * 94U + (trail - 0xA1U);
            const std::uint16_t point = wt::INDEX_JIS0212[pointer];
            if (point == 0U) {
                return false;
            }
            append_utf8(point, out);
            i += 2U;
            continue;
        }
        if (byte >= 0xA1U && byte <= 0xFEU) {
            if (i + 1U >= bytes.size()) {
                return false;
            }
            const unsigned char trail = static_cast<unsigned char>(bytes[i + 1U]);
            if (trail < 0xA1U || trail > 0xFEU) {
                return false;
            }
            const std::uint32_t pointer = (byte - 0xA1U) * 94U + (trail - 0xA1U);
            const std::uint16_t point = wt::INDEX_JIS0208[pointer];
            if (point == 0U) {
                return false;
            }
            append_utf8(point, out);
            i += 1U;
            continue;
        }
        return false;
    }
    return true;
}

bool decode_big5(const std::string& bytes, std::string* out) {
    for (std::size_t i = 0; i < bytes.size(); ++i) {
        const unsigned char byte = static_cast<unsigned char>(bytes[i]);
        if (decode_ascii(byte, out)) {
            continue;
        }
        if (byte < 0x81U || byte > 0xFEU || i + 1U >= bytes.size()) {
            return false;
        }
        const unsigned char trail = static_cast<unsigned char>(bytes[i + 1U]);
        const std::uint32_t trail_offset = trail < 0x7FU ? 0x40U : 0x62U;
        if (!((trail >= 0x40U && trail <= 0x7EU) || (trail >= 0xA1U && trail <= 0xFEU))) {
            return false;
        }
        const std::uint32_t pointer = (byte - 0x81U) * 157U + (trail - trail_offset);
        const std::uint32_t rebased = pointer - wt::BIG5_FIRST_POINTER;
        if (rebased >= sizeof(wt::BIG5_LOW) / sizeof(wt::BIG5_LOW[0])) {
            return false;
        }
        const std::uint16_t low = wt::BIG5_LOW[rebased];
        if (low == 0U) {
            return false;
        }
        const bool astral = (wt::BIG5_ASTRAL[rebased >> 3U] >> (rebased & 7U)) & 1U;
        append_utf8(astral ? (wt::BIG5_ASTRAL_PLANE | low) : low, out);
        i += 1U;
    }
    return true;
}

bool decode_euc_kr(const std::string& bytes, std::string* out) {
    for (std::size_t i = 0; i < bytes.size(); ++i) {
        const unsigned char byte = static_cast<unsigned char>(bytes[i]);
        if (decode_ascii(byte, out)) {
            continue;
        }
        if (byte < 0x81U || byte > 0xFEU || i + 1U >= bytes.size()) {
            return false;
        }
        const unsigned char trail = static_cast<unsigned char>(bytes[i + 1U]);
        if (trail < 0x41U || trail > 0xFEU) {
            return false;
        }
        const std::uint32_t pointer = (byte - 0x81U) * 190U + (trail - 0x41U);
        const std::uint16_t point = wt::INDEX_EUC_KR[pointer];
        if (point == 0U) {
            return false;
        }
        append_utf8(point, out);
        i += 1U;
    }
    return true;
}

bool decode_gb18030(const std::string& bytes, std::string* out) {
    for (std::size_t i = 0; i < bytes.size(); ++i) {
        const unsigned char byte = static_cast<unsigned char>(bytes[i]);
        if (decode_ascii(byte, out)) {
            continue;
        }
        if (byte == 0x80U) {
            // The spec maps a lone 0x80 to the euro sign in its decode path.
            append_utf8(0x20ACU, out);
            continue;
        }
        if (byte < 0x81U || byte > 0xFEU || i + 1U >= bytes.size()) {
            return false;
        }

        // A four-byte sequence has a digit second byte.
        const unsigned char second = static_cast<unsigned char>(bytes[i + 1U]);
        if (second >= 0x30U && second <= 0x39U) {
            if (i + 3U >= bytes.size()) {
                return false;
            }
            const unsigned char third = static_cast<unsigned char>(bytes[i + 2U]);
            const unsigned char fourth = static_cast<unsigned char>(bytes[i + 3U]);
            if (third < 0x81U || third > 0xFEU || fourth < 0x30U || fourth > 0x39U) {
                return false;
            }
            const std::uint32_t pointer = ((byte - 0x81U) * 10U + (second - 0x30U)) * 1260U
                                          + (third - 0x81U) * 10U + (fourth - 0x30U);
            // The four-byte form is defined by the range table, which is
            // sorted by pointer.
            std::size_t low = 0U;
            std::size_t high = sizeof(wt::GB18030_RANGES) / sizeof(wt::GB18030_RANGES[0]);
            if (pointer < wt::GB18030_RANGES[0].pointer) {
                return false;
            }
            while (low + 1U < high) {
                const std::size_t middle = low + (high - low) / 2U;
                if (wt::GB18030_RANGES[middle].pointer <= pointer) {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            const std::uint32_t point =
                wt::GB18030_RANGES[low].code_point + (pointer - wt::GB18030_RANGES[low].pointer);
            if (point > REPLACEMENT_LIMIT) {
                return false;
            }
            append_utf8(point, out);
            i += 3U;
            continue;
        }

        const std::uint32_t trail_offset = second < 0x7FU ? 0x40U : 0x41U;
        if (!((second >= 0x40U && second <= 0x7EU) || (second >= 0x80U && second <= 0xFEU))) {
            return false;
        }
        const std::uint32_t pointer = (byte - 0x81U) * 190U + (second - trail_offset);
        const std::uint16_t point = wt::INDEX_GB18030[pointer];
        if (point == 0U) {
            return false;
        }
        append_utf8(point, out);
        i += 1U;
    }
    return true;
}

// Reads one UTF-8 code point. Returns false on malformed input so the encoder
// never emits bytes for text it cannot represent.
bool next_utf8_code_point(const std::string& text, std::size_t* offset, std::uint32_t* out) {
    const unsigned char first = static_cast<unsigned char>(text[*offset]);
    if (first < 0x80U) {
        *out = first;
        *offset += 1U;
        return true;
    }

    std::size_t width = 0U;
    std::uint32_t code_point = 0U;
    if ((first & 0xE0U) == 0xC0U) {
        width = 2U;
        code_point = first & 0x1FU;
    } else if ((first & 0xF0U) == 0xE0U) {
        width = 3U;
        code_point = first & 0x0FU;
    } else if ((first & 0xF8U) == 0xF0U) {
        width = 4U;
        code_point = first & 0x07U;
    } else {
        return false;
    }
    if (*offset + width > text.size()) {
        return false;
    }
    for (std::size_t k = 1U; k < width; ++k) {
        const unsigned char continuation = static_cast<unsigned char>(text[*offset + k]);
        if ((continuation & 0xC0U) != 0x80U) {
            return false;
        }
        code_point = (code_point << 6) | (continuation & 0x3FU);
    }
    *out = code_point;
    *offset += width;
    return true;
}

bool encode_single_byte(const LegacyEncoding& encoding, const std::string& utf8, std::string* out) {
    std::size_t offset = 0U;
    while (offset < utf8.size()) {
        std::uint32_t code_point = 0U;
        if (!next_utf8_code_point(utf8, &offset, &code_point)) {
            return false;
        }
        if (code_point < 0x80U) {
            out->push_back(static_cast<char>(code_point));
            continue;
        }
        if (code_point > 0xFFFFU) {
            return false;
        }
        for (std::uint32_t index = 0U; index < 128U; ++index) {
            if (wt::SINGLE_BYTE_TABLE[encoding.single_byte_index][index] == code_point) {
                out->push_back(static_cast<char>(0x80U + index));
                code_point = 0U;
                break;
            }
        }
        if (code_point != 0U) {
            return false;
        }
    }
    return true;
}

bool encode_two_byte_pointer(
    const LegacyEncoding& encoding,
    std::uint32_t code_point,
    std::uint32_t* pointer
) {
    switch (encoding.id) {
    case LEGACY_ENCODING_SHIFT_JIS:
        return lookup_pointer(
            wt::ENCODE_JIS0208_POINTS,
            wt::ENCODE_JIS0208_POINTERS,
            sizeof(wt::ENCODE_JIS0208_POINTS) / sizeof(wt::ENCODE_JIS0208_POINTS[0]),
            code_point,
            pointer
        );
    case LEGACY_ENCODING_EUC_JP:
        // EUC-JP uses the unrestricted jis0208 pointer space.
        return lookup_pointer(
            wt::ENCODE_JIS0208_EUC_JP_POINTS,
            wt::ENCODE_JIS0208_EUC_JP_POINTERS,
            sizeof(wt::ENCODE_JIS0208_EUC_JP_POINTS) / sizeof(wt::ENCODE_JIS0208_EUC_JP_POINTS[0]),
            code_point,
            pointer
        );
    case LEGACY_ENCODING_GB18030:
        return lookup_pointer(
            wt::ENCODE_GB18030_POINTS,
            wt::ENCODE_GB18030_POINTERS,
            sizeof(wt::ENCODE_GB18030_POINTS) / sizeof(wt::ENCODE_GB18030_POINTS[0]),
            code_point,
            pointer
        );
    case LEGACY_ENCODING_BIG5:
        return lookup_pointer(
            wt::ENCODE_BIG5_POINTS,
            wt::ENCODE_BIG5_POINTERS,
            sizeof(wt::ENCODE_BIG5_POINTS) / sizeof(wt::ENCODE_BIG5_POINTS[0]),
            code_point,
            pointer
        );
    case LEGACY_ENCODING_EUC_KR:
        return lookup_pointer(
            wt::ENCODE_EUC_KR_POINTS,
            wt::ENCODE_EUC_KR_POINTERS,
            sizeof(wt::ENCODE_EUC_KR_POINTS) / sizeof(wt::ENCODE_EUC_KR_POINTS[0]),
            code_point,
            pointer
        );
    default:
        return false;
    }
}

bool encode_multi_byte(const LegacyEncoding& encoding, const std::string& utf8, std::string* out) {
    std::size_t offset = 0U;
    while (offset < utf8.size()) {
        std::uint32_t code_point = 0U;
        if (!next_utf8_code_point(utf8, &offset, &code_point)) {
            return false;
        }
        if (code_point < 0x80U) {
            out->push_back(static_cast<char>(code_point));
            continue;
        }

        if (encoding.id == LEGACY_ENCODING_SHIFT_JIS && code_point >= 0xFF61U
            && code_point <= 0xFF9FU) {
            out->push_back(static_cast<char>(0xA1U + (code_point - 0xFF61U)));
            continue;
        }
        if (encoding.id == LEGACY_ENCODING_EUC_JP && code_point >= 0xFF61U
            && code_point <= 0xFF9FU) {
            out->push_back(static_cast<char>(0x8EU));
            out->push_back(static_cast<char>(0xA1U + (code_point - 0xFF61U)));
            continue;
        }
        if (encoding.id == LEGACY_ENCODING_GB18030 && code_point == 0x20ACU) {
            out->push_back(static_cast<char>(0x80U));
            continue;
        }

        std::uint32_t pointer = 0U;
        if (!encode_two_byte_pointer(encoding, code_point, &pointer)) {
            if (encoding.id != LEGACY_ENCODING_GB18030) {
                return false;
            }
            // gb18030 falls back to its four-byte form for everything the
            // two-byte index does not cover.
            std::uint32_t pointer_value = 0U;
            std::size_t low = 0U;
            std::size_t high = sizeof(wt::GB18030_RANGES) / sizeof(wt::GB18030_RANGES[0]);
            while (low + 1U < high) {
                const std::size_t middle = low + (high - low) / 2U;
                if (wt::GB18030_RANGES[middle].code_point <= code_point) {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            if (wt::GB18030_RANGES[low].code_point > code_point) {
                return false;
            }
            pointer_value =
                wt::GB18030_RANGES[low].pointer + (code_point - wt::GB18030_RANGES[low].code_point);
            const std::uint32_t byte1 = pointer_value / (10U * 126U * 10U);
            const std::uint32_t remainder = pointer_value % (10U * 126U * 10U);
            const std::uint32_t byte2 = remainder / (126U * 10U);
            const std::uint32_t remainder2 = remainder % (126U * 10U);
            const std::uint32_t byte3 = remainder2 / 10U;
            const std::uint32_t byte4 = remainder2 % 10U;
            out->push_back(static_cast<char>(0x81U + byte1));
            out->push_back(static_cast<char>(0x30U + byte2));
            out->push_back(static_cast<char>(0x81U + byte3));
            out->push_back(static_cast<char>(0x30U + byte4));
            continue;
        }

        switch (encoding.id) {
        case LEGACY_ENCODING_SHIFT_JIS: {
            const std::uint32_t lead = pointer / 188U;
            const std::uint32_t trail = pointer % 188U;
            const std::uint32_t lead_offset = lead < 0x1FU ? 0x81U : 0xC1U;
            const std::uint32_t trail_offset = trail < 0x3FU ? 0x40U : 0x41U;
            out->push_back(static_cast<char>(lead + lead_offset));
            out->push_back(static_cast<char>(trail + trail_offset));
            break;
        }
        case LEGACY_ENCODING_EUC_JP:
            out->push_back(static_cast<char>(pointer / 94U + 0xA1U));
            out->push_back(static_cast<char>(pointer % 94U + 0xA1U));
            break;
        case LEGACY_ENCODING_BIG5: {
            const std::uint32_t lead = pointer / 157U;
            const std::uint32_t trail = pointer % 157U;
            const std::uint32_t trail_offset = trail < 0x3FU ? 0x40U : 0x62U;
            out->push_back(static_cast<char>(lead + 0x81U));
            out->push_back(static_cast<char>(trail + trail_offset));
            break;
        }
        case LEGACY_ENCODING_EUC_KR:
            out->push_back(static_cast<char>(pointer / 190U + 0x81U));
            out->push_back(static_cast<char>(pointer % 190U + 0x41U));
            break;
        case LEGACY_ENCODING_GB18030: {
            const std::uint32_t lead = pointer / 190U;
            const std::uint32_t trail = pointer % 190U;
            const std::uint32_t trail_offset = trail < 0x3FU ? 0x40U : 0x41U;
            out->push_back(static_cast<char>(lead + 0x81U));
            out->push_back(static_cast<char>(trail + trail_offset));
            break;
        }
        default:
            return false;
        }
    }
    return true;
}

} // namespace

bool legacy_encoding_from_label(const std::string& label, LegacyEncoding* out) {
    if (label.empty() || label == "UTF-8" || label == "ASCII" || label == "US-ASCII") {
        out->id = LEGACY_ENCODING_NONE;
        out->single_byte_index = 0U;
        return false;
    }

    if (label == "SHIFT_JIS" || label == "SHIFT-JIS" || label == "SJIS" || label == "CP932") {
        out->id = LEGACY_ENCODING_SHIFT_JIS;
        return true;
    }
    if (label == "EUC-JP" || label == "EUC_JP") {
        out->id = LEGACY_ENCODING_EUC_JP;
        return true;
    }
    if (label == "GB18030" || label == "GBK" || label == "GB2312" || label == "EUC-CN") {
        out->id = LEGACY_ENCODING_GB18030;
        return true;
    }
    if (label == "BIG5" || label == "BIG-5" || label == "BIG5-HKSCS" || label == "CP950") {
        out->id = LEGACY_ENCODING_BIG5;
        return true;
    }
    if (label == "EUC-KR" || label == "EUC_KR" || label == "UHC" || label == "CP949") {
        out->id = LEGACY_ENCODING_EUC_KR;
        return true;
    }

    std::uint32_t index = 0U;
    if (single_byte_index_for_label(label, &index)) {
        out->id = LEGACY_ENCODING_SINGLE_BYTE_BASE + index;
        out->single_byte_index = index;
        return true;
    }
    return false;
}

const char* legacy_encoding_name(const LegacyEncoding& encoding) {
    switch (encoding.id) {
    case LEGACY_ENCODING_SHIFT_JIS:
        return "SHIFT_JIS";
    case LEGACY_ENCODING_EUC_JP:
        return "EUC-JP";
    case LEGACY_ENCODING_GB18030:
        return "GB18030";
    case LEGACY_ENCODING_BIG5:
        return "BIG5";
    case LEGACY_ENCODING_EUC_KR:
        return "EUC-KR";
    default:
        break;
    }
    if (encoding.id >= LEGACY_ENCODING_SINGLE_BYTE_BASE
        && encoding.single_byte_index < wt::SINGLE_BYTE_ENCODING_COUNT) {
        static const char* const CANONICAL[] = {
            "windows-1250", "windows-1251", "windows-1252", "windows-1253", "windows-1254",
            "windows-1255", "windows-1256", "windows-1257", "windows-1258", "windows-874",
            "iso-8859-2",   "iso-8859-3",   "iso-8859-4",   "iso-8859-5",   "iso-8859-6",
            "iso-8859-7",   "iso-8859-8",   "iso-8859-10",  "iso-8859-13",  "iso-8859-14",
            "iso-8859-15",  "iso-8859-16",  "koi8-r",       "koi8-u",       "macintosh",
            "ibm866",
        };
        return CANONICAL[encoding.single_byte_index];
    }
    return "unknown";
}

bool legacy_encoding_decode(
    const LegacyEncoding& encoding,
    const std::string& bytes,
    std::string* utf8_out
) {
    utf8_out->clear();
    utf8_out->reserve(bytes.size());

    if (encoding.id >= LEGACY_ENCODING_SINGLE_BYTE_BASE) {
        if (encoding.single_byte_index >= wt::SINGLE_BYTE_ENCODING_COUNT) {
            return false;
        }
        for (std::size_t i = 0; i < bytes.size(); ++i) {
            const unsigned char byte = static_cast<unsigned char>(bytes[i]);
            const std::uint16_t point = decode_single_byte(encoding, byte);
            if (point == 0U && byte != 0U) {
                return false;
            }
            append_utf8(point, utf8_out);
        }
        return true;
    }

    switch (encoding.id) {
    case LEGACY_ENCODING_SHIFT_JIS:
        return decode_shift_jis(bytes, utf8_out);
    case LEGACY_ENCODING_EUC_JP:
        return decode_euc_jp(bytes, utf8_out);
    case LEGACY_ENCODING_GB18030:
        return decode_gb18030(bytes, utf8_out);
    case LEGACY_ENCODING_BIG5:
        return decode_big5(bytes, utf8_out);
    case LEGACY_ENCODING_EUC_KR:
        return decode_euc_kr(bytes, utf8_out);
    default:
        return false;
    }
}

bool legacy_encoding_encode(
    const LegacyEncoding& encoding,
    const std::string& utf8,
    std::string* bytes_out
) {
    bytes_out->clear();
    bytes_out->reserve(utf8.size());

    if (encoding.id >= LEGACY_ENCODING_SINGLE_BYTE_BASE) {
        if (encoding.single_byte_index >= wt::SINGLE_BYTE_ENCODING_COUNT) {
            return false;
        }
        std::string encoded;
        if (!encode_single_byte(encoding, utf8, &encoded)) {
            return false;
        }
        bytes_out->swap(encoded);
        return true;
    }
    if (encoding.id == LEGACY_ENCODING_NONE) {
        return false;
    }

    std::string encoded;
    if (!encode_multi_byte(encoding, utf8, &encoded)) {
        return false;
    }
    bytes_out->swap(encoded);
    return true;
}

#endif
