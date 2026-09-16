#!/usr/bin/env python3
"""Independent Python oracles for the Palace prop pixel formats.

This is a *second implementation*, written from the reference sources rather than
from the Rust, so that `diff_oracle.py` can compare two independent readings of
the same format. Two reference readings are supported:

``openpalace``
    A line-by-line port of ``PalaceProp.as`` (the ActionScript client). This is the
    authority for what a real client renders, including its floating-point dither
    (``255/63`` and ``255/31`` as ``Number`` values) and its hard-coded 44x44 /
    1936-pixel loop counts.

``taj``
    A port of Taj's ``PalaceProp.cs``, which differs from the ActionScript in
    exactly the places noted in the crate README: it folds the dither into integer
    constants (``255/63`` -> 4, ``255/31`` -> 8), shortens the 20-bit loop to 967
    groups, and never implemented 16-bit. Selecting this style quantifies those
    divergences instead of hiding them.

The third oracle used by ``diff_oracle.py`` is the pre-existing
``~/palace-corpus/tools/prop_decoder.py``, which only understands 8-bit props.

Output is the same ``PRGBA1`` container the Rust tool writes: an 8-byte magic,
little-endian ``uint32`` width, little-endian ``uint32`` height, then raw
top-down RGBA8 - so a differential is a plain byte comparison.
"""

from __future__ import annotations

import re
import struct
import sys
import zlib
from pathlib import Path

DITHER_20BIT = 255 / 63
DITHER_S20BIT = 255 / 31
SIXTEEN_BIT_MASK = 0xFFC1

FORMAT_8 = "8-bit"
FORMAT_16 = "16-bit"
FORMAT_20 = "20-bit"
FORMAT_S20 = "s20-bit"
FORMAT_32 = "32-bit"


def load_palette(as3_path: str) -> list[int]:
    """Extract ``clutARGB`` from PalacePalette.as.

    Identical in behaviour to ``prop_decoder.py``'s loader, so a palette
    difference can never be mistaken for a decoder difference.
    """
    text = Path(as3_path).read_text(errors="replace")
    start = text.index("clutARGB:Array = [")
    chunk = text[start : text.index("];", start)]
    chunk = re.sub(r"//[^\n]*", "", chunk)
    chunk = re.sub(r"/\*.*?\*/", "", chunk, flags=re.S)
    values = []
    for token in re.findall(r"0x[0-9a-fA-F]+|-?\d+", chunk):
        value = int(token, 16) if token.lower().startswith("0x") else int(token, 10)
        values.append(value & 0xFFFFFFFF)
    return values[:256]


def parse_header(data: bytes):
    """Return ``(width, height, flags, endian, format)`` or raise ValueError."""
    if len(data) < 12:
        raise ValueError("short header")
    little = data[1] == 0
    endian = "<" if little else ">"
    width, height, _h, _v, _s, flags = struct.unpack_from(f"{endian}6h", data, 0)
    flags &= 0xFFFF
    if (flags & SIXTEEN_BIT_MASK) == 0xFF80:
        fmt = FORMAT_16
    elif flags & 0x200:
        fmt = FORMAT_S20
    elif flags & 0x100:
        fmt = FORMAT_32
    elif flags & 0x40:
        fmt = FORMAT_20
    else:
        fmt = FORMAT_8
    return width, height, flags, endian, fmt


def _argb_to_rgba(words: list[int], width: int, height: int) -> bytes:
    out = bytearray(width * height * 4)
    for i, word in enumerate(words[: width * height]):
        out[i * 4 + 0] = (word >> 16) & 0xFF
        out[i * 4 + 1] = (word >> 8) & 0xFF
        out[i * 4 + 2] = word & 0xFF
        out[i * 4 + 3] = (word >> 24) & 0xFF
    return bytes(out)


def decode_openpalace(data: bytes, palette: list[int]):
    """Port of PalaceProp.as `renderBitmap` + the five `decode*BitProp` methods."""
    width, height, _flags, _endian, fmt = parse_header(data)
    pixels = width * height
    if fmt == FORMAT_8:
        payload = data[12:]
        buffer = [0] * (width * (height + 1))
        index = width
        n = 0
        counter = 0
        for _y in range(height - 1, -1, -1):
            x = width
            while x > 0:
                # The reference indexes a ByteArray, where a read past the end
                # yields 0 rather than raising. Modelling that is what makes this
                # oracle agree with the client about *why* a prop is bad: a
                # truncated stream reads zeroes, stops making progress, and trips
                # the runaway guard below.
                cb = payload[n] if n < len(payload) else 0
                n += 1
                mc = cb >> 4
                pc = cb & 0xF
                x -= mc + pc
                if x < 0:
                    raise ValueError("8-bit RLE row overflow")
                counter += 1
                if counter > 6000:
                    raise ValueError("8-bit RLE runaway (reference guard)")
                index += mc
                while pc > 0:
                    pc -= 1
                    if len(payload) > n:
                        buffer[index] = palette[payload[n] & 0xFF]
                        index += 1
                        n += 1
        return width, height, _argb_to_rgba(buffer[width : width + pixels], width, height), fmt

    raw = zlib.decompress(data[12:])
    words = []
    if fmt == FORMAT_32:
        for x in range(1936 if pixels == 1936 else pixels):
            o = x * 4
            r, g, b, a = raw[o], raw[o + 1], raw[o + 2], raw[o + 3]
            words.append((a << 24) | (r << 16) | (g << 8) | b)
    elif fmt == FORMAT_S20:
        groups = (pixels + 1) // 2
        for x in range(groups):
            o = x * 5
            r = int(((raw[o] >> 3) & 31) * DITHER_S20BIT) & 0xFF
            c = (raw[o] << 8) | raw[o + 1]
            g = int(((c >> 6) & 31) * DITHER_S20BIT) & 0xFF
            b = int(((c >> 1) & 31) * DITHER_S20BIT) & 0xFF
            c = (raw[o + 1] << 8) | raw[o + 2]
            a = int(((c >> 4) & 31) * DITHER_S20BIT) & 0xFF
            words.append((a << 24) | (r << 16) | (g << 8) | b)
            c = (raw[o + 2] << 8) | raw[o + 3]
            r = int(((c >> 7) & 31) * DITHER_S20BIT) & 0xFF
            g = int(((c >> 2) & 31) * DITHER_S20BIT) & 0xFF
            c = (raw[o + 3] << 8) | raw[o + 4]
            b = int(((c >> 5) & 31) * DITHER_S20BIT) & 0xFF
            a = int((c & 31) * DITHER_S20BIT) & 0xFF
            words.append((a << 24) | (r << 16) | (g << 8) | b)
    elif fmt == FORMAT_20:
        groups = (pixels + 1) // 2
        for x in range(groups):
            o = x * 5
            r = int((((raw[o] >> 2) & 63)) * DITHER_20BIT)
            c = (raw[o] << 8) | raw[o + 1]
            g = int(((c >> 4) & 63) * DITHER_20BIT)
            c = (raw[o + 1] << 8) | raw[o + 2]
            b = int(((c >> 6) & 63) * DITHER_20BIT)
            a = ((c >> 4) & 3) * 85
            words.append((a << 24) | (r << 16) | (g << 8) | b)
            c = (raw[o + 2] << 8) | raw[o + 3]
            r = int(((c >> 6) & 63) * DITHER_20BIT)
            g = int((c & 63) * DITHER_20BIT)
            c = raw[o + 4]
            b = int(((c >> 2) & 63) * DITHER_20BIT)
            a = (c & 3) * 85
            words.append((a << 24) | (r << 16) | (g << 8) | b)
    elif fmt == FORMAT_16:
        for x in range(1936 if pixels == 1936 else pixels):
            o = x * 2
            c = (raw[o] * 256) | raw[o + 1]
            r = int(((raw[o] // 8) & 31) * 255 / 31) & 0xFF
            g = int(((c // 64) & 31) * 255 / 31) & 0xFF
            b = int(((c // 2) & 31) * 255 / 31) & 0xFF
            a = (c & 1) * 255 & 0xFF
            words.append((a << 24) | (r << 16) | (g << 8) | b)
    return width, height, _argb_to_rgba(words, width, height), fmt


def decode_taj(data: bytes, palette: list[int]):
    """Port of Taj's PalaceProp.cs, including its divergences from ActionScript."""
    width, height, _flags, _endian, fmt = parse_header(data)
    if fmt == FORMAT_16:
        raise NotImplementedError("Taj never implemented 16-bit")
    if fmt == FORMAT_8:
        return decode_openpalace(data, palette)

    # Taj trims 12 bytes, seeks 2 more (raw deflate), then loops.
    body = data[12:]
    raw = zlib.decompress(body)
    dither20 = 255 // 63
    dither_s20 = 255 // 31
    words = []
    if fmt == FORMAT_32:
        for x in range(1936):
            o = x * 4
            r, g, b, a = raw[o], raw[o + 1], raw[o + 2], raw[o + 3]
            words.append((a << 24) | (r << 16) | (g << 8) | b)
    elif fmt == FORMAT_S20:
        for x in range(968):
            o = x * 5
            r = (((raw[o] >> 3) & 31) * dither_s20) & 0xFF
            c = (raw[o] << 8) | raw[o + 1]
            g = ((c >> 6) & 31) * dither_s20 & 0xFF
            b = ((c >> 1) & 31) * dither_s20 & 0xFF
            c = (raw[o + 1] << 8) | raw[o + 2]
            a = ((c >> 4) & 31) * dither_s20 & 0xFF
            words.append((a << 24) | (r << 16) | (g << 8) | b)
            c = (raw[o + 2] << 8) | raw[o + 3]
            r = ((c >> 7) & 31) * dither_s20 & 0xFF
            g = ((c >> 2) & 31) * dither_s20 & 0xFF
            c = (raw[o + 3] << 8) | raw[o + 4]
            b = ((c >> 5) & 31) * dither_s20 & 0xFF
            a = (c & 31) * dither_s20 & 0xFF
            words.append((a << 24) | (r << 16) | (g << 8) | b)
    elif fmt == FORMAT_20:
        for x in range(967):
            o = x * 5
            r = ((raw[o] >> 2) & 63) * dither20
            c = (raw[o] << 8) | raw[o + 1]
            g = ((c >> 4) & 63) * dither20
            c = (raw[o + 1] << 8) | raw[o + 2]
            b = ((c >> 6) & 63) * dither20
            a = ((c >> 4) & 3) * 85
            words.append((a << 24) | (r << 16) | (g << 8) | b)
            c = (raw[o + 2] << 8) | raw[o + 3]
            r = ((c >> 6) & 63) * dither20
            g = (c & 63) * dither20
            c = raw[o + 4]
            b = ((c >> 2) & 63) * dither20
            a = (c & 3) * 85
            words.append((a << 24) | (r << 16) | (g << 8) | b)
    return width, height, _argb_to_rgba(words, width, height), fmt


def wrap_rgba(width: int, height: int, rgba: bytes) -> bytes:
    return b"PRGBA1" + struct.pack("<II", width, height) + rgba


def main(argv: list[str]) -> int:
    if len(argv) < 5:
        print(
            "usage: oracle_prop.py <openpalace|taj> <PalacePalette.as> <prop.bin> <out.rgba>",
            file=sys.stderr,
        )
        return 2
    style, palette_path, prop_path, out_path = argv[1:5]
    palette = load_palette(palette_path)
    data = Path(prop_path).read_bytes()
    decode = decode_openpalace if style == "openpalace" else decode_taj
    width, height, rgba, fmt = decode(data, palette)
    Path(out_path).write_bytes(wrap_rgba(width, height, rgba))
    print(f"{prop_path}: {width}x{height} {fmt} style={style}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
