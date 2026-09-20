#!/usr/bin/env python3
"""Regenerate the golden `.prp` fixtures under `fixtures/prp/`.

The fixtures are intentionally built from the small, already-checked-in real prop
blobs in `fixtures/*.bin`, so a fixture is a few hundred bytes to a few kilobytes
and its pixels are pinned by `tests/fixtures.rs`.

This is the *canonical* writer: records within a type are sorted ascending by
signed 32-bit id, blobs are packed contiguously, and every record's CRC is the
Palace asset CRC of `blob[12:]`. That is the shape the pserver format requires
(PRP-FORMAT.md §5), and it is deliberately *not* the shape of the real 1999-2008
collections in `$MEDIA/Prop Files/`, which are unordered. See
`fixtures/prp/README.md` and `.omo/evidence/task-5-python-oracle.txt`.

Run:  python3 crates/palace-prop/validation/make_prp_fixtures.py
"""
from __future__ import annotations

import hashlib
import os
import struct

HERE = os.path.dirname(os.path.abspath(__file__))
FIXTURES = os.path.normpath(os.path.join(HERE, "..", "fixtures"))
OUT = os.path.join(FIXTURES, "prp")

PROP = 0x50726F70  # "Prop" as a big-endian 4CC, stored little-endian
FAVE = 0x46617665  # "Fave"

MAGIC = 0xD9216290


def asset_crc(data: bytes) -> int:
    crc = MAGIC
    for b in data:
        crc = (((crc << 1) | (crc >> 31)) & 0xFFFFFFFF) ^ b
    return crc


def signed(x: int) -> int:
    return x - 0x100000000 if x >= 0x80000000 else x


def blob_bytes(name: str) -> bytes:
    with open(os.path.join(FIXTURES, name), "rb") as fh:
        return fh.read()


class Entry:
    __slots__ = ("rid", "blob", "last_use", "name", "flags")

    def __init__(self, rid, blob, last_use=0, name=None, flags=0):
        self.rid = rid
        self.blob = blob
        self.last_use = last_use
        self.name = name  # bytes or None -> nameOffset = -1
        self.flags = flags


def build(types):
    """types: list of (tag, [Entry, ...]) in the order they must appear.

    Entries are NOT re-sorted here: the caller passes them in the exact signed-id
    order to pin, which keeps duplicate (id, crc) variants deterministic.
    """
    data_region = bytearray()
    names = bytearray()
    recs = []
    type_meta = []
    for tag, entries in types:
        first = len(recs)
        for e in entries:
            off = len(data_region)
            data_region += e.blob
            if e.name is None:
                name_off = 0xFFFFFFFF
            else:
                name_off = len(names)
                names.append(len(e.name))
                names += e.name
            crc = asset_crc(e.blob[12:]) if len(e.blob) >= 12 else 0
            recs.append((e.rid & 0xFFFFFFFF, 0, off, len(e.blob),
                         e.last_use, name_off, e.flags, crc))
        type_meta.append((tag, len(entries), first))

    n_types = len(type_meta)
    n_assets = len(recs)
    types_off = 24
    recs_off = types_off + n_types * 12
    names_off = recs_off + n_assets * 32
    map_size = names_off + len(names)

    out = bytearray()
    out += struct.pack("<IIII", 16, len(data_region), 16 + len(data_region), map_size)
    out += data_region
    out += struct.pack("<IIIIII", n_types, n_assets, len(names),
                       types_off, recs_off, names_off)
    for tag, n, first in type_meta:
        out += struct.pack("<III", tag, n, first)
    for rec in recs:
        out += struct.pack("<IIIIIIII", *rec)
    out += names
    assert len(out) == (16 + len(data_region)) + map_size
    return bytes(out)


def sha16(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()[:16]


def main():
    os.makedirs(OUT, exist_ok=True)

    head = blob_bytes("8bit_head_rare.bin")       # 227 B, 8-bit
    s20 = blob_bytes("s20_bit.bin")               # 40 B, S20, all transparent
    b20 = blob_bytes("20bit_bit.bin")             # 1075 B, 20-bit
    b32 = blob_bytes("32bit_bit.bin")             # 991 B, 32-bit
    avatar = blob_bytes("8bit_avatar.bin")        # 2036 B, 8-bit

    files = {}

    # 1. single_type.prp: one "Prop" type, no Fave. One negative (high-bit) id
    #    before a positive one, to pin signed ordering.
    files["single_type.prp"] = build([
        (PROP, [
            Entry(0x803D6C40, b20, last_use=1234, name=b"Onstage"),
            Entry(0x3A3AD1F7, b32, last_use=99, name=b"Look Down"),
        ]),
    ])

    # 2. prop_fave.prp: a normal Prop plus the Fave size-0 sentinel.
    files["prop_fave.prp"] = build([
        (PROP, [Entry(7, avatar, last_use=42, name=b"Smile")]),
        (FAVE, [Entry(128, b"", name=None)]),
    ])

    # 3. dup_id_crc.prp: one id carried by THREE crc variants, adjacent, in the
    #    exact input order, plus a second id whose crc equals the first variant.
    files["dup_id_crc.prp"] = build([
        (PROP, [
            Entry(1000, head, name=b"Variant A"),
            Entry(1000, s20, name=b"Variant B"),
            Entry(1000, b20, name=b"Variant C"),
            Entry(1001, head, name=None),
        ]),
    ])

    # 4. empty.prp: a valid file with a Prop type and zero records.
    files["empty.prp"] = build([(PROP, [])])

    # 5. unnamed_records.prp: every record has nameOffset = -1; no names blob.
    files["unnamed_records.prp"] = build([
        (PROP, [Entry(300, b32), Entry(301, s20)]),
    ])

    # 6. long_nonascii_name.prp: a 198-byte latin1 name mixing high bytes.
    long_name = b"Caf\xe9 \xf1and\xfa \xdcber-" + b"\xe9" * 180
    files["long_nonascii_name.prp"] = build([
        (PROP, [Entry(55, avatar, name=long_name)]),
    ])

    for name, data in sorted(files.items()):
        path = os.path.join(OUT, name)
        with open(path, "wb") as fh:
            fh.write(data)
        print(f"wrote {name:28s} {len(data):6d} bytes  sha256={hashlib.sha256(data).hexdigest()}")


if __name__ == "__main__":
    main()
