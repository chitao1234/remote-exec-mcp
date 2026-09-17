#!/usr/bin/env python3
"""Generate the C++ legacy-encoding conversion tables from the WHATWG indexes.

The WHATWG Encoding Standard publishes the index tables every conforming
decoder and encoder is defined against. Generating from that source rather
than from a local codec avoids a real class of bug: CPython's cp932 and cp949
accept thousands of byte sequences the standard's Shift_JIS and EUC-KR reject,
so tables transcribed from Python would decode files the Rust daemon rejects.

Usage:
    scripts/generate_encoding_tables.py <output.h>

The pinned digest below is checked before anything is written, so an upstream
change is a build-time decision rather than a silent behavior change.
"""

import hashlib
import json
import sys
import urllib.request
from pathlib import Path

INDEXES_URL = "https://encoding.spec.whatwg.org/indexes.json"
INDEXES_SHA256 = "b7c8961095f0b4cae8f4a16a5011e7c84047a60c1277e28ee8df9e56074ee509"

# The legacy single-byte encodings the daemon can decode. Each maps one byte
# to one code point, so a single 128-entry table suffices per encoding: bytes
# below 0x80 are the ASCII identity.
SINGLE_BYTE = (
    "windows-1250",
    "windows-1251",
    "windows-1252",
    "windows-1253",
    "windows-1254",
    "windows-1255",
    "windows-1256",
    "windows-1257",
    "windows-1258",
    "windows-874",
    "iso-8859-2",
    "iso-8859-3",
    "iso-8859-4",
    "iso-8859-5",
    "iso-8859-6",
    "iso-8859-7",
    "iso-8859-8",
    "iso-8859-10",
    "iso-8859-13",
    "iso-8859-14",
    "iso-8859-15",
    "iso-8859-16",
    "koi8-r",
    "koi8-u",
    "macintosh",
    "ibm866",
)

# Detector label -> canonical index name. Only labels the WHATWG standard
# itself folds onto another index appear here; every other label must match a
# SINGLE_BYTE entry directly. Folding a label onto a genuinely different
# encoding would silently corrupt every byte it mis-maps.
SINGLE_BYTE_ALIASES = {
    # The spec decodes iso-8859-1 and latin1 as windows-1252.
    "ISO-8859-1": "windows-1252",
    # The spec decodes iso-8859-9 as windows-1254.
    "ISO-8859-9": "windows-1254",
    # The spec decodes logical Hebrew as iso-8859-8.
    "ISO-8859-8-I": "iso-8859-8",
    # The spec decodes TIS-620 as windows-874.
    "TIS-620": "windows-874",
    "ISO-8859-11": "windows-874",
    # 866 and CP866 are the same code page.
    "CP866": "ibm866",
    # KOI8-RU is the spec's koi8-u.
    "KOI8-RU": "koi8-u",
    # uchardet reports MacRoman and MacCyrillic; only MacRoman is in the spec.
    "MACINTOSH": "macintosh",
    "MACROMAN": "macintosh",
}

# Multi-byte encodings. gb18030 additionally needs the four-byte range table.
MULTI_BYTE = ("jis0208", "jis0212", "big5", "euc-kr", "gb18030")


def fetch_indexes():
    with urllib.request.urlopen(INDEXES_URL, timeout=60) as response:
        raw = response.read()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != INDEXES_SHA256:
        raise SystemExit(
            f"{INDEXES_URL} has sha256 {digest}, not the pinned {INDEXES_SHA256}; "
            "review the upstream change, then update the pin and the generated header"
        )
    parsed = json.loads(raw)
    ranges = [(pointer, point) for pointer, point in parsed["gb18030-ranges"]]
    indexes = {name: table for name, table in parsed.items() if name != "gb18030-ranges"}
    return indexes, ranges


def rows(values, per_line=12):
    lines = []
    for start in range(0, len(values), per_line):
        chunk = values[start : start + per_line]
        lines.append("    " + ", ".join(f"0x{value:04X}" for value in chunk) + ",")
    return "\n".join(lines)


def emit_single_byte(indexes):
    """Byte -> code point for the high half of each single-byte encoding.

    0 means the byte is unmapped in that encoding, which the decoder reports
    as a decode error rather than silently substituting.
    """
    tables = []
    for name in SINGLE_BYTE:
        table = indexes[name]
        values = [0 if point is None else point for point in table]
        if len(values) != 128:
            raise SystemExit(f"{name} has {len(values)} entries, expected 128")
        tables.append((name, values))

    out = ["/* Byte -> code point for bytes 0x80..0xFF, per legacy single-byte encoding. */"]
    out.append("enum SingleByteEncodingIndex {")
    for position, (name, _) in enumerate(tables):
        out.append(f"    SINGLE_BYTE_{name.upper().replace('-', '_')} = {position},")
    out.append("};")
    out.append("")
    out.append(
        f"const std::uint16_t SINGLE_BYTE_TABLE[SINGLE_BYTE_ENCODING_COUNT][128] = {{"
    )
    for name, values in tables:
        out.append(f"    {{ /* {name} */")
        out.append(rows(values))
        out.append("    },")
    out.append("};")
    out.append("")

    # Alias rows, so a detector label resolves to a table slot without the
    # decoder knowing any encoding by name.
    positions = {name: position for position, (name, _) in enumerate(tables)}
    unknown = sorted(set(SINGLE_BYTE_ALIASES.values()) - set(positions))
    if unknown:
        raise SystemExit(f"aliases reference unknown indexes: {unknown}")

    out.append("struct SingleByteAlias {")
    out.append("    const char* label;")
    out.append("    std::uint32_t table_index;")
    out.append("};")
    out.append("")
    alias_entries = sorted(SINGLE_BYTE_ALIASES.items())
    out.append(f"const SingleByteAlias SINGLE_BYTE_ALIASES[{len(alias_entries)}] = {{")
    for label, target in alias_entries:
        out.append(f'    {{"{label}", SINGLE_BYTE_{target.upper().replace("-", "_")}}},')
    out.append("};")
    return "\n".join(out)


def emit_flat(name, index):
    """Pointer -> code point for a two-byte index, in its native pointer space."""
    values = [0 if point is None else point for point in index]
    astral = [value for value in values if value > 0xFFFF]
    if astral:
        raise SystemExit(f"index {name} has astral code points; it needs a wider table type")
    return (
        f"/* index {name}: pointer -> code point. 0 marks an unmapped pointer. */\n"
        f"const std::uint16_t INDEX_{name.upper().replace('-', '_')}[{len(values)}] = {{\n"
        f"{rows(values)}\n"
        "};"
    )


def emit_big5(index):
    """Big5 pointers below 942 are all unmapped, so the table is rebased.

    Code points above the BMP all live in plane 2, so a bitmap of flags
    reconstructs them without widening every entry.
    """
    first = 942
    tail = index[first:]
    low = [0 if point is None else (point & 0xFFFF) for point in tail]
    bitmap = bytearray((len(tail) + 7) // 8)
    for pointer, point in enumerate(tail):
        if point is not None and point > 0xFFFF:
            if point >> 16 != 2:
                raise SystemExit(f"Big5 pointer {pointer + first} maps outside plane 2")
            bitmap[pointer // 8] |= 1 << (pointer % 8)
    bitmap_bytes = (len(tail) + 7) // 8
    bits = "\n".join(
        "    " + ", ".join(f"0x{byte:02X}" for byte in bitmap[at : at + 16]) + ","
        for at in range(0, len(bitmap), 16)
    )
    return (
        f"const std::uint32_t BIG5_FIRST_POINTER = {first}U;\n"
        "const std::uint32_t BIG5_ASTRAL_PLANE = 0x20000U;\n\n"
        "/* Low sixteen bits of the code point for BIG5_FIRST_POINTER + i; 0 is unmapped. */\n"
        f"const std::uint16_t BIG5_LOW[{len(low)}] = {{\n{rows(low)}\n}};\n\n"
        "/* Bit i is set when BIG5_LOW[i] needs BIG5_ASTRAL_PLANE added back. */\n"
        f"const std::uint8_t BIG5_ASTRAL[{bitmap_bytes}] = {{\n{bits}\n}};"
    )


def emit_encode_index(name, index, skip_pointers=frozenset()):
    """Code point -> pointer, as two parallel sorted arrays, for the encoders.

    The spec derives encoder pointers from the decoder indexes with per-index
    rules (Shift_JIS excludes a pointer range, Big5 excludes the low pointers
    and prefers the last duplicate for six code points). Applying those rules
    here keeps the encoder a binary search rather than a re-implementation of
    each encoding's quirks at run time.

    Points and pointers are emitted as separate arrays because a struct per
    entry roughly triples the generated source without changing the data.
    """
    pairs = {}
    for pointer, point in enumerate(index):
        if point is None or pointer in skip_pointers:
            continue
        # The spec's index pointer for a code point is the first entry for it.
        if point not in pairs:
            pairs[point] = pointer

    entries = sorted(pairs.items())
    return _emit_encode_arrays(name, entries)


def _emit_encode_arrays(name, entries):
    upper = name.upper().replace("-", "_")
    largest = max((pointer for _, pointer in entries), default=0)
    if largest > 0xFFFF:
        raise SystemExit(
            f"{name} has pointer {largest}, which does not fit the 16-bit pointer array"
        )
    points = "\n".join(
        "    " + ", ".join(f"0x{point:04X}U" for point, _ in entries[at : at + 10]) + ","
        for at in range(0, len(entries), 10)
    )
    pointers = "\n".join(
        "    " + ", ".join(f"{pointer}U" for _, pointer in entries[at : at + 14]) + ","
        for at in range(0, len(entries), 14)
    )
    return (
        f"/* Sorted code points and their matching pointers for {name}. */\n"
        f"const std::uint32_t ENCODE_{upper}_POINTS[{len(entries)}] = {{\n{points}\n}};\n\n"
        f"const std::uint16_t ENCODE_{upper}_POINTERS[{len(entries)}] = {{\n{pointers}\n}};"
    )


def emit_big5_encode(index):
    """Big5 encoding follows the spec's own index Big5 pointer algorithm.

    Pointers below the Hong Kong Supplementary Character Set range are
    excluded, and six code points use their last pointer instead of the first
    because those positions are the ones the standard defines.
    """
    first = 942
    last_pointer = {}
    pairs = {}
    for pointer, point in enumerate(index):
        if point is None or pointer < first:
            continue
        last_pointer[point] = pointer
        if point not in pairs:
            pairs[point] = pointer
    # U+2550, U+255E, U+2561, U+256A, U+5341, U+5345 take their last pointer.
    for point in (0x2550, 0x255E, 0x2561, 0x256A, 0x5341, 0x5345):
        if point in last_pointer:
            pairs[point] = last_pointer[point]

    return _emit_encode_arrays("big5", sorted(pairs.items()))


def emit_ranges(ranges):
    lines = "\n".join(
        f"    {{0x{pointer:04X}U, 0x{point:04X}U}}," for pointer, point in ranges
    )
    return (
        "struct Gb18030Range {\n"
        "    std::uint32_t pointer;\n"
        "    std::uint32_t code_point;\n"
        "};\n\n"
        f"const Gb18030Range GB18030_RANGES[{len(ranges)}] = {{\n{lines}\n}};"
    )


def generate(out_path):
    indexes, ranges = fetch_indexes()

    parts = [
        "/* Auto-generated by scripts/generate_encoding_tables.py - do not edit. */",
        "/*",
        f" * WHATWG Encoding Standard index tables ({INDEXES_URL},",
        f" * sha256 {INDEXES_SHA256}).",
        " *",
        " * The index data is Copyright WHATWG (Apple, Google, Mozilla, Microsoft)",
        " * and is licensed BSD-3-Clause. See third_party/whatwg-encoding/LICENSE.",
        " */",
        "",
        "#pragma once",
        "",
        "#include <cstdint>",
        "",
        "namespace whatwg_tables {",
        "",
        f"const std::size_t SINGLE_BYTE_ENCODING_COUNT = {len(SINGLE_BYTE)}U;",
        "",
        emit_single_byte(indexes),
        "",
    ]

    for name in MULTI_BYTE:
        if name == "big5":
            parts.append(emit_big5(indexes[name]))
            parts.append("")
            parts.append(emit_big5_encode(indexes[name]))
        else:
            parts.append(emit_flat(name, indexes[name]))
            parts.append("")
            if name == "jis0208":
                # Shift_JIS excludes the pointer range the spec reserves, so
                # its encoder index drops pointers 8272..8835.
                parts.append(
                    emit_encode_index(
                        name, indexes[name], skip_pointers=frozenset(range(8272, 8836))
                    )
                )
                parts.append("")
                # EUC-JP indexes jis0208 in its native pointer space, so it
                # needs its own encoder index without the Shift_JIS exclusion.
                parts.append(emit_encode_index("jis0208_euc_jp", indexes[name]))
            else:
                parts.append(emit_encode_index(name, indexes[name]))
        parts.append("")

    parts.append(emit_ranges(ranges))
    parts.append("")
    parts.append("} // namespace whatwg_tables")
    parts.append("")

    out_path.write_text("\n".join(parts), encoding="utf-8")
    print(f"wrote {out_path}: {out_path.stat().st_size // 1024} KiB")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {sys.argv[0]} <output.h>")
    generate(Path(sys.argv[1]))
