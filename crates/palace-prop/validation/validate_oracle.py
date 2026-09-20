#!/usr/bin/env python3
"""Validate `$CORPUS/tools/rebuild_prp.py` as a byte-exact `.prp` oracle.

Two questions are answered, per file in `$MEDIA/Prop Files/`:

  PURE     Re-serialise the records and names already in the file, with the
           merge sources disabled. This isolates rebuild_prp.py's *writer*: does
           it reproduce the input bytes? This is the only mode in which a
           round-trip is even meaningful.
  DEFAULT  Run the tool as shipped, with its harvested/archive-mined blob pool.
           This is a *merge*, so it injects ids the file does not contain; the
           injection count is reported per file to show that a round-trip is
           impossible by construction.

Every original is copied to the work directory first and its SHA-256 is checked
after the run; the originals are never opened for writing.

Usage:
    python3 crates/palace-prop/validation/validate_oracle.py [--default-cmp]

`--default-cmp` additionally runs the real default merge on three representative
files and prints the first divergent byte (it is slower and writes ~150 MB per
file, deleted immediately).
"""
from __future__ import annotations

import argparse
import contextlib
import gc
import glob
import hashlib
import importlib.util
import io
import os
import shutil
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from diff_prp import locate  # noqa: E402

TOOL = "$CORPUS/tools/rebuild_prp.py"
SOURCE_DIR = "$HOME/Pictures/Prop Files"
WORK = "/tmp/work/prp-oracle"
DEFAULT_CMP_SUBSET = ["Palace - Hidden.PRP", "idk.prp", "Taz-Purged.prp"]


def load_tool():
    spec = importlib.util.spec_from_file_location("rebuild_prp", TOOL)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def sha256(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def first_diff(a: bytes, b: bytes):
    n = min(len(a), len(b))
    for i in range(n):
        if a[i] != b[i]:
            return i, a[i], b[i]
    if len(a) != len(b):
        return n, None, None
    return None, None, None


def sources():
    found = []
    for pat in ("*.prp", "*.PRP"):
        found += glob.glob(os.path.join(SOURCE_DIR, pat))
    return sorted(set(found))


def snapshot(sources):
    dest = os.path.join(WORK, "originals")
    os.makedirs(dest, exist_ok=True)
    before = {}
    for s in sources:
        before[s] = sha256(s)
        shutil.copy2(s, os.path.join(dest, os.path.basename(s)))
    return dest, before


def verify_originals(before):
    changed = [s for s, h in before.items() if sha256(s) != h]
    return changed


def run_pure(mod, src):
    mod.BLOB_DIRS = []
    mod.ARK_DIR = "/nonexistent-ark"
    mod.ARCHIVE_DIRS = []
    mod.load_blobs = lambda: {}
    mod.load_archives = lambda: {}
    out = os.path.join(WORK, "out-pure", os.path.basename(src) + ".out")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    mod.SRC, mod.OUT = src, out
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            mod.main()
        a = open(src, "rb").read()
        b = open(out, "rb").read()
        off, exp, act = first_diff(a, b)
        if off is None:
            return ("PASS", len(a), None, None, None)
        return ("FAIL", len(a), off, exp, locate(a, off))
    except Exception as exc:  # noqa: BLE001 - report, never abort the sweep
        return ("ERROR", os.path.getsize(src), None, f"{type(exc).__name__}: {exc}", None)
    finally:
        try:
            os.unlink(out)
        except OSError:
            pass
        gc.collect()


def existing_ids(path):
    d = open(path, "rb").read()
    _, _, mo, _ = struct.unpack_from("<IIII", d, 0)
    _nT, nA, _nN, _tO, rO, _nO = struct.unpack_from("<iiiiii", d, mo)
    ids = set()
    for i in range(nA):
        rid, _rh, _doff, dsize, _lt, _no, _fl, _cr = struct.unpack_from(
            "<IIIIIIII", d, mo + rO + i * 32
        )
        if rid == 0x80 and dsize == 0:
            continue
        ids.add(rid)
    return ids


def run_default_cmp(mod, blobs, mined, subset):
    mod.load_blobs = lambda: blobs
    mod.load_archives = lambda: mined
    outdir = os.path.join(WORK, "out-default")
    os.makedirs(outdir, exist_ok=True)
    rows = []
    for src in sources():
        name = os.path.basename(src)
        if name not in subset:
            continue
        out = os.path.join(outdir, name + ".out")
        mod.SRC, mod.OUT = src, out
        row = [name, ""]
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                mod.main()
            a = open(src, "rb").read()
            b = open(out, "rb").read()
            off, exp, act = first_diff(a, b)
            if off is None:
                row[1] = f"PASS ({len(a)} bytes)"
            else:
                row[1] = (f"FAIL first={off:#x} exp={exp:#04x} got={act:#04x} "
                          f"size {len(a)}->{len(b)}")
        except Exception as exc:  # noqa: BLE001
            row[1] = f"ERROR {type(exc).__name__}: {exc}"
        finally:
            try:
                os.unlink(out)
            except OSError:
                pass
            gc.collect()
        rows.append(row)
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--default-cmp", action="store_true",
                    help="also run the real merge on three representative files")
    args = ap.parse_args()

    srcs = sources()
    os.makedirs(WORK, exist_ok=True)
    _, before = snapshot(srcs)
    print(f"# rebuild_prp.py oracle validation")
    print(f"tool    : {TOOL}")
    print(f"files   : {len(srcs)} .prp/.PRP in {SOURCE_DIR}")
    print(f"copies  : {os.path.join(WORK, 'originals')}")
    print()

    print("## PURE re-serialisation (merge sources disabled)")
    print(f"{'file':24s} {'bytes':>10s} {'result':7s} {'first':>7s}  location")
    pure_fail = 0
    mod = load_tool()
    for s in srcs:
        status, size, off, detail, loc = run_pure(mod, s)
        if status == "PASS":
            print(f"{os.path.basename(s):24s} {size:10d} {'PASS':7s} {'-':>7s}")
        else:
            pure_fail += 1
            if status == "FAIL":
                print(f"{os.path.basename(s):24s} {size:10d} {'FAIL':7s} {off:#7x}  {loc}")
            else:
                print(f"{os.path.basename(s):24s} {size:10d} {'ERROR':7s} {'-':>7s}  {detail}")
    print(f"pure round-trips: {len(srcs) - pure_fail}/{len(srcs)}")
    print()

    print("## CONTROL: canonical synthetic fixtures (would round-trip if exact)")
    fixdir = os.path.normpath(os.path.join(HERE, "..", "fixtures", "prp"))
    controls = [f for f in sorted(glob.glob(os.path.join(fixdir, "*.prp")))
                if not os.path.basename(f).startswith("real_")]
    for f in controls:
        status, size, off, detail, loc = run_pure(mod, f)
        if status == "PASS":
            print(f"{os.path.basename(f):28s} {size:6d} PASS")
        else:
            shown = f"{off:#x}" if status == "FAIL" else detail
            print(f"{os.path.basename(f):28s} {size:6d} {status:5s} first={shown}")
    print()

    print("## DEFAULT merge injection (pool disables round-trip by construction)")
    mod = load_tool()
    blobs = mod.load_blobs()
    mined = mod.load_archives()
    pool = set(blobs) | set(mined)
    print(f"harvested blobs={len(blobs)} archive-mined={len(mined)} union ids={len(pool)}")
    print(f"{'file':24s} {'existing_ids':>12s} {'pool_adds':>10s} {'output_ids':>10s}")
    for s in srcs:
        ex = existing_ids(s)
        print(f"{os.path.basename(s):24s} {len(ex):12d} {len(pool - ex):10d} {len(ex | pool):10d}")
    print()

    if args.default_cmp:
        print("## DEFAULT merge actual run/cmp (representative subset)")
        for name, result in run_default_cmp(mod, blobs, mined, set(DEFAULT_CMP_SUBSET)):
            print(f"{name:24s} {result}")
        print()

    changed = verify_originals(before)
    print("## Originals unchanged")
    if changed:
        print("CHANGED (this is a bug):")
        for s in changed:
            print(f"  {s}")
    else:
        print(f"all {len(srcs)} originals byte-identical before/after (sha256 verified)")
    print()
    print("VERDICT: rebuild_prp.py is NOT a byte-exact round-trip oracle for these")
    print("collections. See the notes in the report for the three structural reasons.")
    return 1 if (pure_fail or changed) else 0


if __name__ == "__main__":
    raise SystemExit(main())
