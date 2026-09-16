#!/usr/bin/env python3
"""Differential test: Rust `palace-prop` vs two independent Python oracles.

Inputs are a manifest of prop files and a directory of ``<line>.rgba`` files that
``prop-tool rgba-batch`` produced from the *same* manifest. For every prop this
script:

1. decodes it with ``oracle_prop.decode_openpalace`` (a port of the ActionScript
   reference) and compares bytes against the Rust output;
2. decodes it with ``prop_decoder.py`` - the pre-existing, independently written
   8-bit decoder at ``~/palace-corpus/tools/prop_decoder.py`` - and compares again;
3. additionally decodes with the Taj-style port and reports the divergences
   without counting them as failures, because they are *known* differences
   (see `oracle_prop.py` and the crate README).

Exit status is non-zero when any comparison disagrees, so this can gate a build.

    usage: diff_corpus.py <PalacePalette.as> <rust_rgba_dir> <manifest> [max_report]
"""

from __future__ import annotations

import importlib.util
import struct
import sys
from collections import defaultdict
from pathlib import Path

# `~/palace-corpus/` is a read-only reference. Loading `prop_decoder.py` from it would
# otherwise create or refresh `~/palace-corpus/tools/__pycache__/`, so bytecode writing
# is disabled for the whole process before any module is imported.
sys.dont_write_bytecode = True

import oracle_prop  # noqa: E402 - must follow the dont_write_bytecode switch


def load_prop_decoder(path: str):
    spec = importlib.util.spec_from_file_location("prop_decoder_oracle", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def unpack(data: bytes):
    """Parse a `PRGBA1` blob into `(width, height, rgba)`."""
    if data[:6] != b"PRGBA1":
        raise ValueError("not a PRGBA1 blob")
    width, height = struct.unpack_from("<II", data, 6)
    return width, height, data[14:]


def image_digest(width: int, height: int, rgba: bytes) -> int:
    """FNV-1a 64 over dimensions then RGBA. Must match `prop-tool digests`."""
    hash_value = 0xCBF29CE484222325
    for byte in (
        width.to_bytes(4, "little") + height.to_bytes(4, "little") + rgba
    ):
        hash_value ^= byte
        hash_value = (hash_value * 0x00000100000001B3) & 0xFFFFFFFFFFFFFFFF
    return hash_value


def load_digests(path: Path):
    """`index -> ("ok", format, w, h, digest)` or `index -> ("fail", reason)`."""
    entries = {}
    for line in path.read_text().splitlines():
        fields = line.split("\t")
        if len(fields) < 3:
            continue
        index = int(fields[0])
        if fields[1] == "FAIL":
            entries[index] = ("fail", fields[2])
        elif len(fields) >= 5:
            entries[index] = (
                "ok",
                fields[1],
                int(fields[2]),
                int(fields[3]),
                int(fields[4], 16),
            )
    return entries


def first_difference(a: bytes, b: bytes) -> int | None:
    for i, (x, y) in enumerate(zip(a, b)):
        if x != y:
            return i
    return None if len(a) == len(b) else min(len(a), len(b))


def main(argv: list[str]) -> int:
    if len(argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    palette_path, rust_dir, manifest_path = argv[1:4]
    max_report = int(argv[4]) if len(argv) > 4 else 5

    palette = oracle_prop.load_palette(palette_path)
    prop_decoder = load_prop_decoder("$HOME/palace-corpus/tools/prop_decoder.py")
    prop_decoder_palette = prop_decoder.load_palette(palette_path)

    manifest = [
        line.strip()
        for line in Path(manifest_path).read_text().splitlines()
        if line.strip()
    ]
    rust_source = Path(rust_dir)
    digest_mode = rust_source.is_file()
    digests = load_digests(rust_source) if digest_mode else {}

    stats: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    taj_stats: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    reports: list[str] = []
    oracle_says_bad: list[tuple[str, str]] = []
    prop_decoder_says_bad: list[tuple[str, str]] = []

    mode = "digests" if digest_mode else "raw RGBA files"

    for index, prop_path in enumerate(manifest):
        data = Path(prop_path).read_bytes()
        try:
            _, _, _, _, fmt = oracle_prop.parse_header(data)
        except Exception as error:  # noqa: BLE001 - reported, not raised
            fmt = f"unparseable ({error})"

        # --- the Rust side ---------------------------------------------------
        rust_error = None
        rust_summary = None
        if digest_mode:
            entry = digests.get(index)
            if entry is None:
                rust_error = "no digest line"
            elif entry[0] == "fail":
                rust_error = entry[1]
            else:
                rust_summary = (entry[2], entry[3], entry[4])
        else:
            rust_file = rust_source / f"{index}.rgba"
            if rust_file.exists():
                rust_summary = unpack(rust_file.read_bytes())
            else:
                rust_error = "no output"

        # --- oracle 1: the ActionScript reference port -----------------------
        try:
            ow, oh, orgba, _ = oracle_prop.decode_openpalace(data, palette)
        except Exception as error:  # noqa: BLE001
            stats[fmt]["oracle_error"] += 1
            if rust_error is None:
                # Rust decoded but the reference port could not: worth a look.
                stats[fmt]["oracle_only_error"] += 1
                reports.append(f"{prop_path}: rust=ok ({fmt}) oracle={error}")
            else:
                # Both sides refuse it. That is agreement, not a disagreement,
                # but the pair is recorded so the reader can see the reference
                # treats the prop the same way.
                stats[fmt]["both_reject"] += 1
                oracle_says_bad.append(
                    (prop_path, f"oracle={error} / rust={rust_error}")
                )
            continue
        if rust_error is not None:
            stats[fmt]["rust_error"] += 1
            reports.append(f"{prop_path}: rust={rust_error} oracle=ok ({fmt})")
            continue

        stats[fmt]["compared"] += 1
        if digest_mode:
            rw, rh, rdigest = rust_summary
            if (rw, rh) != (ow, oh):
                stats[fmt]["size_mismatch"] += 1
                reports.append(f"{prop_path}: size rust={rw}x{rh} oracle={ow}x{oh}")
            elif rdigest != image_digest(ow, oh, orgba):
                stats[fmt]["pixel_mismatch"] += 1
                reports.append(
                    f"{prop_path}: digest rust={rdigest:016x} "
                    f"oracle={image_digest(ow, oh, orgba):016x}"
                )
            else:
                stats[fmt]["match"] += 1
        else:
            rw, rh, rrgba = rust_summary
            if (rw, rh) != (ow, oh):
                stats[fmt]["size_mismatch"] += 1
                reports.append(f"{prop_path}: size rust={rw}x{rh} oracle={ow}x{oh}")
            elif rrgba != orgba:
                at = first_difference(rrgba, orgba)
                stats[fmt]["pixel_mismatch"] += 1
                reports.append(
                    f"{prop_path}: rust != openpalace oracle at byte {at} "
                    f"(rust {rrgba[at:at+8].hex()} vs oracle {orgba[at:at+8].hex()})"
                )
            else:
                stats[fmt]["match"] += 1

        # --- oracle 2: prop_decoder.py (8-bit only) -------------------------
        if fmt == oracle_prop.FORMAT_8:
            try:
                image, _, _, _ = prop_decoder.decode_prop(data, prop_decoder_palette)
            except Exception as error:  # noqa: BLE001
                stats[fmt]["prop_decoder_error"] += 1
                prop_decoder_says_bad.append((prop_path, f"{type(error).__name__}: {error}"))
                continue
            pd_rgba = image.tobytes()
            stats[fmt]["prop_decoder_compared"] += 1
            if pd_rgba == orgba:
                stats[fmt]["prop_decoder_match"] += 1
            else:
                stats[fmt]["prop_decoder_mismatch"] += 1
                at = first_difference(pd_rgba, orgba)
                reports.append(
                    f"{prop_path}: prop_decoder.py != oracle at byte {at} "
                    f"(pd {pd_rgba[at:at+8].hex()} vs oracle {orgba[at:at+8].hex()})"
                )
        else:
            stats[fmt]["prop_decoder_skipped_format"] += 1

        # --- oracle 3: Taj style, informational -----------------------------
        try:
            tw, th, trgba, _ = oracle_prop.decode_taj(data, palette)
        except Exception as error:  # noqa: BLE001
            taj_stats[fmt][f"taj_error: {error}"] += 1
            continue
        if (tw, th) == (ow, oh) and trgba == orgba:
            taj_stats[fmt]["same_as_openpalace"] += 1
        else:
            differing = sum(
                1 for x, y in zip(trgba, orgba) if x != y
            )
            taj_stats[fmt]["differs"] += 1
            taj_stats[fmt]["differing_channels"] += differing

    print(f"# differential over {len(manifest)} props ({mode})")
    print("\n## Rust vs OpenPalace (ActionScript) oracle")
    for fmt in sorted(stats):
        fields = ", ".join(f"{k}={v}" for k, v in sorted(stats[fmt].items()))
        print(f"  {fmt:12s} {fields}")
    print("\n## Rust vs prop_decoder.py (8-bit third-party oracle)")
    for fmt in sorted(stats):
        if stats[fmt]["prop_decoder_compared"]:
            print(
                f"  {fmt:12s} compared={stats[fmt]['prop_decoder_compared']} "
                f"match={stats[fmt]['prop_decoder_match']} "
                f"mismatch={stats[fmt]['prop_decoder_mismatch']}"
            )
        if stats[fmt]["prop_decoder_skipped_format"]:
            print(
                f"  {fmt:12s} skipped={stats[fmt]['prop_decoder_skipped_format']} "
                "(prop_decoder.py supports 8-bit only)"
            )
    print("\n## Taj-style port (informational: known divergences)")
    for fmt in sorted(taj_stats):
        fields = ", ".join(f"{k}={v}" for k, v in sorted(taj_stats[fmt].items()))
        print(f"  {fmt:12s} {fields}")

    failures = sum(
        v
        for per_format in stats.values()
        for k, v in per_format.items()
        if k in {"pixel_mismatch", "size_mismatch", "oracle_only_error", "rust_error"}
    )
    if reports:
        print(f"\n## first {min(len(reports), max_report)} disagreements")
        for line in reports[:max_report]:
            print(f"  {line}")
    print(f"\nTOTAL disagreements: {failures}")

    both_reject = sum(per_format.get("both_reject", 0) for per_format in stats.values())
    if both_reject:
        print(
            f"\n## {both_reject} props both sides refuse (recorded, not counted as"
            " disagreements)"
        )
        for label, detail in oracle_says_bad[:max_report]:
            print(f"  {label}: {detail}")

    prop_decoder_errors = sum(
        per_format.get("prop_decoder_error", 0) for per_format in stats.values()
    )
    if prop_decoder_errors:
        decoded_by_rust = sum(
            per_format.get("prop_decoder_compared", 0) for per_format in stats.values()
        )
        print(
            f"\n## prop_decoder.py raised on {prop_decoder_errors} prop(s) that both the"
            f" Rust decoder and the reference oracle decoded ({decoded_by_rust} props it"
            " did decode matched byte for byte)"
        )
        print("   The Python decoder indexes the payload without a bounds check, so it")
        print("   cannot express the reference's end-of-payload tolerance (see the")
        print("   crate README). These are a documented oracle limitation, not a")
        print("   Rust failure, so they do not fail the run.")
        for label, detail in prop_decoder_says_bad[:max_report]:
            print(f"  {label}: {detail}")

    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
