#!/usr/bin/env python3
"""Independent validator/reader for a Palace `.prp` prop roster.

This tool is deliberately independent of the Rust writer: it was written from
`PRP-FORMAT.md` (sections 2, 3.1, 4, 5, 7 and the pitfalls checklist in 10),
not from the crate's code and not from `rebuild_prp.py` (which the project
disproved as a byte-exact oracle: it re-sorts, repacks and appends a sentinel,
so it cannot verify someone else's writer).

What it does, in plain terms: it opens a `.prp`, walks the container exactly the
way the format document describes, and reports every record it finds together
with the CRC of that record's pixel payload — recomputed here from the bytes,
using the reference client's rotate-left XOR algorithm.

Only records of the `"Prop"` type are CRC-checked, because that is exactly what
the server's `ValidateProps` does (PRP-FORMAT.md section 6: it iterates types and
compares `0x50726f70` before hashing). `"Fave"`, `"User"`, `"IUsr"` and any other
type are *not* payloads — the `Fave` sentinel's `crc` field is not a payload CRC
at all (section 7) — so their CRCs are reported as unvalidated, never as errors.
A file is ACCEPTed when the container is self-consistent and every `Prop` payload
hashes to the CRC the record declares. The exit status is 0 for ACCEPT and 1 for
REJECT, so a test harness (Rust or shell) can rely on it.

Usage:
    python3 independent_reader.py [--verbose] [--max-records N] path/to/roster.prp

Output:
    Human-readable lines, then a `result: ACCEPT` / `result: REJECT` summary.
    `--verbose` prints every record instead of the first few.

The format (from PRP-FORMAT.md):

    +0   AssetFileHeader   u32 dataOffset, u32 dataSize,
                           u32 assetMapOffset, u32 assetMapSize        (16 B)
    +16  data region       concatenated prop blobs
    ...  AssetMapHeader    s32 nbrTypes, s32 nbrAssets, s32 lenNames,
                           u32 typesOffset, u32 recsOffset, u32 namesOffset
         AssetTypeRec[..]  12 B each: s32 4CC, s32 count, s32 firstIndex
         AssetRec[..]      32 B each: id, rHandle, dataOffset, dataSize,
                           lastUseTime, nameOffset, flags, crc
         names blob        [len:u8][latin-1 bytes]...

    Invariant: filesize == assetMapOffset + assetMapSize.
    A record's blob lives at file offset 16 + dataOffset (NOT dataOffset).
    The CRC covers blob[12:] — the payload only, never the 12-byte header.
"""

import struct
import sys

# The client's CRC seed (PRP-FORMAT.md section 4).
ASSET_CRC_MAGIC = 0xD9216290

# Asset type 4CCs, read as a big-endian integer (section 2.3).
PROP = 0x50726F70
FAVE = 0x46617665
USER = 0x55736572
IUSR = 0x49557372

TYPE_NAMES = {
    PROP: "Prop",
    FAVE: "Fave",
    USER: "User",
    IUSR: "IUsr",
}

FILE_HEADER_LEN = 16
MAP_HEADER_LEN = 24
TYPE_REC_LEN = 12
ASSET_REC_LEN = 32
PROP_HEADER_LEN = 12


def asset_crc(payload):
    """The reference ASSET_CRC_MAGIC checksum over a payload slice.

    One pass, rotate-left-carry then XOR with the next byte, everything masked
    back to 32 bits (PRP-FORMAT.md section 4).
    """
    crc = ASSET_CRC_MAGIC
    for byte in payload:
        crc = ((crc << 1) | (crc >> 31)) & 0xFFFFFFFF
        crc ^= byte
    return crc


def fourcc_name(raw):
    """Render a stored 4CC value the way the format stores it."""
    if raw is None:
        return "untyped"
    name = TYPE_NAMES.get(raw)
    if name is not None:
        return name
    packed = struct.pack("<I", raw)
    text = "".join(chr(b) if 32 <= b < 127 else "?" for b in packed)
    return 'unknown("{}")'.format(text)


def prop_header_fields(blob):
    """The six s16 words of a prop blob header, or None when short."""
    if len(blob) < PROP_HEADER_LEN:
        return None
    return struct.unpack_from("<hhhhhh", blob, 0)


class RosterValidator:
    """Parse and validate one `.prp`, collecting errors instead of raising."""

    def __init__(self, path, buf):
        self.path = path
        self.buf = buf
        self.errors = []
        self.warnings = []
        self.types = []
        self.records = []
        self.names = []
        self.crc_checked = 0
        self.crc_failures = 0
        self.crc_unvalidated = 0

    # -- reporting ---------------------------------------------------------

    def error(self, message):
        self.errors.append(message)

    def warn(self, message):
        self.warnings.append(message)

    # -- checks ------------------------------------------------------------

    def run(self):
        buf = self.buf
        if len(buf) < FILE_HEADER_LEN:
            self.error(
                "file is {} byte(s) long; at least {} are needed for the file "
                "header".format(len(buf), FILE_HEADER_LEN)
            )
            return

        data_offset, data_size, map_offset, map_size = struct.unpack_from(
            "<IIII", buf, 0
        )
        self.data_offset = data_offset
        self.data_size = data_size
        self.map_offset = map_offset
        self.map_size = map_size

        # Section 2 layout rules.
        if data_offset != FILE_HEADER_LEN:
            self.error(
                "dataOffset is {} but the header is {} bytes (spec 2.1)".format(
                    data_offset, FILE_HEADER_LEN
                )
            )
        if map_offset != data_offset + data_size:
            self.error(
                "assetMapOffset {} != dataOffset {} + dataSize {}".format(
                    map_offset, data_offset, data_size
                )
            )
        if len(buf) != map_offset + map_size:
            self.error(
                "size invariant violated: filesize {} != assetMapOffset {} + "
                "assetMapSize {}".format(len(buf), map_offset, map_size)
            )
        if map_offset > len(buf) or map_size > len(buf):
            self.error("the asset map lies past the end of the file")
            return

        map_end = map_offset + map_size
        map_region = buf[map_offset:map_end]
        if len(map_region) < MAP_HEADER_LEN:
            self.error(
                "asset map is {} byte(s) long; the map header alone needs "
                "{}".format(len(map_region), MAP_HEADER_LEN)
            )
            return

        n_types, n_assets, len_names = struct.unpack_from("<iii", map_region, 0)
        types_offset, recs_offset, names_offset = struct.unpack_from(
            "<III", map_region, 12
        )
        if n_types < 0 or n_assets < 0 or len_names < 0:
            self.error("a map header count is negative")
            return

        # Section 2.2: the map-relative offsets are derived, so they must agree.
        if types_offset != MAP_HEADER_LEN:
            self.error(
                "typesOffset is {} but must be {} (map header size)".format(
                    types_offset, MAP_HEADER_LEN
                )
            )
        if recs_offset != MAP_HEADER_LEN + n_types * TYPE_REC_LEN:
            self.error(
                "recsOffset {} != typesOffset {} + {} types * {} bytes".format(
                    recs_offset, MAP_HEADER_LEN, n_types, TYPE_REC_LEN
                )
            )
        if names_offset != recs_offset + n_assets * ASSET_REC_LEN:
            self.error(
                "namesOffset {} != recsOffset {} + {} records * {} bytes".format(
                    names_offset, recs_offset, n_assets, ASSET_REC_LEN
                )
            )

        def section(offset, length, what):
            end = offset + length
            if offset > len(map_region) or end > len(map_region):
                self.error(
                    "{} section runs past the declared map "
                    "({}..{} vs {} bytes)".format(what, offset, end, len(map_region))
                )
                return None
            return map_region[offset:end]

        types_region = section(types_offset, n_types * TYPE_REC_LEN, "type table")
        recs_region = section(recs_offset, n_assets * ASSET_REC_LEN, "record table")
        names_region = section(names_offset, len_names, "names blob")
        if types_region is None or recs_region is None or names_region is None:
            return

        self.parse_types(types_region, n_types, n_assets)
        self.parse_names(names_region)
        self.parse_records(recs_region, n_assets)

    def parse_types(self, region, n_types, n_assets):
        total = 0
        for index in range(n_types):
            asset_type, count, first = struct.unpack_from(
                "<iii", region, index * TYPE_REC_LEN
            )
            self.types.append((asset_type, count, first))
            total += count
            if count < 0 or first < 0:
                self.error("type {} has a negative count or first index".format(index))
                continue
            if first + count > n_assets:
                self.error(
                    "type {} range {}..{} exceeds the {} declared records".format(
                        index, first, first + count, n_assets
                    )
                )
        if total != n_assets:
            self.error(
                "the type table declares {} record(s) but the map header says "
                "{}".format(total, n_assets)
            )
        if not any(t[0] == PROP for t in self.types):
            self.warn("no \"Prop\" type is present")

    def parse_names(self, region):
        at = 0
        end = len(region)
        while at < end:
            length = region[at]
            start = at + 1
            stop = start + length
            if stop > end:
                self.error(
                    "names blob entry at offset {} is truncated ({} byte name, "
                    "only {} available)".format(at, length, end - start)
                )
                break
            name = region[start:stop].decode("latin-1")
            self.names.append((at, name))
            at = stop

    def type_for_record(self, index):
        """The raw 4CC of the type whose declared range covers this record.

        Returns None when no type range covers it (a malformed map); such a
        record is left unvalidated rather than guessed at.
        """
        for asset_type, count, first in self.types:
            if count > 0 and first <= index < first + count:
                return asset_type
        return None

    def parse_records(self, region, n_assets):
        by_offset = {offset: name for offset, name in self.names}
        data_start = self.data_offset
        data_end = self.data_offset + self.data_size

        for index in range(n_assets):
            at = index * ASSET_REC_LEN
            (
                rid,
                r_handle,
                blob_offset,
                blob_size,
                last_use,
                name_offset,
                flags,
                crc,
            ) = struct.unpack_from("<iiIIiiII", region, at)

            blob_start = data_start + blob_offset
            blob_stop = blob_start + blob_size
            blob = None
            if blob_start < data_start or blob_stop > data_end:
                self.error(
                    "record {} (id {}) blob {}..{} falls outside the data region "
                    "{}..{}".format(index, rid, blob_start, blob_stop, data_start, data_end)
                )
            else:
                blob = self.buf[blob_start:blob_stop]

            # ValidateProps (spec section 6) hashes "Prop" records only. Every
            # other type is a different kind of entry (the Fave sentinel's crc
            # is not even a payload CRC), so it is counted as unvalidated and
            # must not be reported as an error.
            record_type = self.type_for_record(index)
            is_prop = record_type == PROP

            payload_crc = None
            if is_prop and blob is not None and blob_size >= PROP_HEADER_LEN:
                payload_crc = asset_crc(blob[PROP_HEADER_LEN:])
                self.crc_checked += 1
                if payload_crc != crc:
                    self.crc_failures += 1
                    self.error(
                        "record {} (id {}) CRC mismatch: stored {:#010x}, "
                        "computed {:#010x} over blob[12:]".format(
                            index, rid, crc & 0xFFFFFFFF, payload_crc
                        )
                    )
            elif is_prop and blob is not None and blob_size > 0:
                # A 1..11 byte "Prop" blob cannot carry the 12-byte header, so
                # its CRC cannot be checked.
                self.warn(
                    "record {} (id {}) has dataSize {} — shorter than the {} byte "
                    "prop header, CRC not checked".format(
                        index, rid, blob_size, PROP_HEADER_LEN
                    )
                )
            elif not is_prop:
                self.crc_unvalidated += 1

            if r_handle != 0:
                self.warn(
                    "record {} (id {}) has rHandle {} on disk; the format says "
                    "it is always 0".format(index, rid, r_handle)
                )
            if flags != 0:
                self.warn(
                    "record {} (id {}) has runtime flags {:#x} on disk; the "
                    "format says they are 0".format(index, rid, flags & 0xFFFFFFFF)
                )
            if name_offset >= 0 and name_offset not in by_offset:
                self.error(
                    "record {} (id {}) nameOffset {} does not point at a name "
                    "entry".format(index, rid, name_offset)
                )

            self.records.append(
                {
                    "index": index,
                    "id": rid,
                    "type": record_type,
                    "data_offset": blob_offset,
                    "data_size": blob_size,
                    "last_use": last_use,
                    "name_offset": name_offset,
                    "name": by_offset.get(name_offset) if name_offset >= 0 else None,
                    "flags": flags,
                    "crc": crc & 0xFFFFFFFF,
                    "payload_crc": payload_crc,
                    "blob": blob,
                }
            )

        # Section 5: each type's records must ascend by signed id. A violation
        # is a lookup corruption on the real server, so it is worth saying out
        # loud even though it is not a structural failure of the container.
        for type_index, (asset_type, count, first) in enumerate(self.types):
            if count <= 1 or first + count > len(self.records):
                continue
            ids = [self.records[i]["id"] for i in range(first, first + count)]
            if ids != sorted(ids):
                self.warn(
                    "type {} ({}) is not sorted by signed id; the server binary-"
                    "searches it".format(type_index, fourcc_name(asset_type))
                )

    # -- output ------------------------------------------------------------

    def describe(self, verbose, max_records):
        out = []
        out.append("independent_reader: {}".format(self.path))
        out.append(
            "file: {} byte(s)  dataOffset={} dataSize={} assetMapOffset={} "
            "assetMapSize={}".format(
                len(self.buf),
                self.data_offset,
                self.data_size,
                self.map_offset,
                self.map_size,
            )
        )
        for index, (asset_type, count, first) in enumerate(self.types):
            out.append(
                'type[{}]: "{}" nbrAssets={} firstAsset={}'.format(
                    index, fourcc_name(asset_type), count, first
                )
            )
        shown = 0
        for record in self.records:
            if not verbose and shown >= max_records:
                out.append(
                    "... {} more record(s); pass --verbose to print them "
                    "all".format(len(self.records) - shown)
                )
                break
            shown += 1
            header = None
            if record["blob"] is not None:
                header = prop_header_fields(record["blob"])
            shape = ""
            if header is not None:
                width, height, h_off, v_off, _script, prop_flags = header
                shape = " prop={}x{} offsets=({},{}) flags={:#06x}".format(
                    width, height, h_off, v_off, prop_flags & 0xFFFF
                )
            crc_state = "CRC OK"
            if record["payload_crc"] is None:
                if record["type"] == PROP:
                    crc_state = "CRC skipped (no 12-byte header)"
                else:
                    crc_state = "CRC not validated (type {})".format(
                        fourcc_name(record["type"])
                    )
            elif record["payload_crc"] != record["crc"]:
                crc_state = "CRC MISMATCH"
            out.append(
                "record[{}]: type={} id={} dataOffset={} dataSize={} name={!r} "
                "crc={:#010x} payload_crc={} {}{}".format(
                    record["index"],
                    fourcc_name(record["type"]),
                    record["id"],
                    record["data_offset"],
                    record["data_size"],
                    record["name"],
                    record["crc"],
                    "none" if record["payload_crc"] is None else format(record["payload_crc"], "#010x"),
                    crc_state,
                    shape,
                )
            )
        out.append("names: {} entr(y/ies)".format(len(self.names)))
        for offset, name in self.names[:max_records]:
            out.append("  name[{}] = {!r}".format(offset, name))
        if len(self.names) > max_records:
            out.append("  ... {} more name(s)".format(len(self.names) - max_records))
        for warning in self.warnings:
            out.append("warning: {}".format(warning))
        for error in self.errors:
            out.append("ERROR: {}".format(error))
        out.append("types: {}".format(len(self.types)))
        out.append("records: {}".format(len(self.records)))
        out.append("names: {}".format(len(self.names)))
        out.append("crc_checked: {}".format(self.crc_checked))
        out.append("crc_failures: {}".format(self.crc_failures))
        out.append("crc_unvalidated: {}".format(self.crc_unvalidated))
        out.append("errors: {}".format(len(self.errors)))
        out.append("warnings: {}".format(len(self.warnings)))
        out.append("result: {}".format("ACCEPT" if not self.errors else "REJECT"))
        return "\n".join(out)


def main(argv):
    verbose = False
    max_records = 20
    paths = []
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg == "--verbose":
            verbose = True
        elif arg == "--max-records":
            index += 1
            if index >= len(argv):
                sys.stderr.write("--max-records needs a number\n")
                return 2
            max_records = int(argv[index])
        elif arg in ("-h", "--help"):
            sys.stdout.write(__doc__)
            return 0
        elif arg.startswith("-"):
            sys.stderr.write("unknown option: {}\n".format(arg))
            return 2
        else:
            paths.append(arg)
        index += 1

    if len(paths) != 1:
        sys.stderr.write("usage: independent_reader.py [--verbose] <file.prp>\n")
        return 2

    path = paths[0]
    try:
        with open(path, "rb") as handle:
            buf = handle.read()
    except OSError as error:
        sys.stderr.write("cannot read {}: {}\n".format(path, error))
        return 2

    validator = RosterValidator(path, buf)
    validator.run()
    sys.stdout.write(validator.describe(verbose, max_records) + "\n")
    return 0 if not validator.errors else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
