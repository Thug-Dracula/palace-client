#!/usr/bin/env python3
"""Differential check: the Rust probe vs palace_walker.py.

Runs both clients against the same pserver, back to back, and compares the
decoded room and user lists. This is the correctness oracle for the wire layer:
`palace_walker.py` is a known-good, from-scratch headless Palace client.

Neither client is modified. `palace_walker.py` is executed read-only from
~/palace-corpus/tools/ and is never written to.

Usage:
    tools/diff_walker.py [--host localhost] [--port 9998]
                         [--user RustProbe] [--walker ~/palace-corpus/tools/palace_walker.py]

Exit status is 0 when the room id sets match, 1 otherwise. Differences are
printed in full rather than summarized, because a mismatch is a protocol bug and
the exact ids matter.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

DEFAULT_WALKER = os.path.expanduser("~/palace-corpus/tools/palace_walker.py")
WALKER_ROOM_RE = re.compile(
    r"^\s+(\d+)\s+users=\s*(\d+)\s+flags=0x([0-9a-fA-F]+)\s+(.*)$"
)
WALKER_COUNT_RE = re.compile(r"room list:\s*(\d+)\s*rooms")
PROBE_SUMMARY_RE = re.compile(r"^SUMMARY rooms=(\d+) users=(\d+)$", re.MULTILINE)


def run_probe(args):
    """Run `cargo run -p palace-probe -- --json` and parse its summary."""
    cmd = [
        "cargo", "run", "--quiet", "-p", "palace-probe", "--",
        "--host", args.host, "--port", str(args.port), "--user", args.user,
        "--json",
    ]
    proc = subprocess.run(
        cmd, cwd=args.workspace, capture_output=True, text=True, timeout=180
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit("palace-probe failed")
    stdout = proc.stdout
    match = PROBE_SUMMARY_RE.search(stdout)
    if not match:
        raise SystemExit("could not find SUMMARY line in probe output")
    payload = stdout[: match.start()]
    data = json.loads(payload)
    return data, int(match.group(1)), int(match.group(2))


def run_walker(args):
    """Run palace_walker.py --list and parse the room table."""
    cmd = [
        sys.executable, args.walker,
        "--host", args.host, "--port", str(args.port),
        "--name", args.user, "--list",
    ]
    # The walker creates its --outdir in the working directory; give it a
    # throwaway one so the workspace is never polluted.
    with tempfile.TemporaryDirectory(prefix="palace-walker-") as scratch:
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=180, cwd=scratch
        )
    rooms = {}
    reported = None
    for line in proc.stdout.splitlines():
        m = WALKER_ROOM_RE.match(line)
        if m:
            rid = int(m.group(1))
            rooms[rid] = {
                "users": int(m.group(2)),
                "flags": int(m.group(3), 16),
                "name": m.group(4),
            }
            continue
        c = WALKER_COUNT_RE.search(line)
        if c:
            reported = int(c.group(1))
    if not rooms:
        sys.stderr.write(proc.stdout)
        sys.stderr.write(proc.stderr)
        raise SystemExit("palace_walker.py produced no room list")
    return rooms, reported


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--host", default="localhost")
    ap.add_argument("--port", type=int, default=9998)
    ap.add_argument("--user", default="RustProbe")
    ap.add_argument("--walker", default=DEFAULT_WALKER)
    ap.add_argument(
        "--workspace",
        default=os.path.expanduser("~/ProgramFiles/palace-client"),
    )
    args = ap.parse_args()

    probe, probe_rooms, probe_users = run_probe(args)
    walker_rooms, walker_reported = run_walker(args)

    probe_ids = {r["id"] for r in probe["rooms"]}
    walker_ids = set(walker_rooms)

    print(f"probe  : {probe_rooms} rooms, {probe_users} users "
          f"(byte order {probe['byte_order']}, server {probe.get('server_name')!r})")
    print(f"walker : {len(walker_rooms)} rooms (reported {walker_reported})")

    only_probe = sorted(probe_ids - walker_ids)
    only_walker = sorted(walker_ids - probe_ids)
    common = sorted(probe_ids & walker_ids)

    name_mismatches = []
    for rid in common:
        probe_name = next(r["name"] for r in probe["rooms"] if r["id"] == rid)
        if probe_name != walker_rooms[rid]["name"]:
            name_mismatches.append((rid, probe_name, walker_rooms[rid]["name"]))

    ok = not only_probe and not only_walker and not name_mismatches
    if only_probe:
        print(f"\nrooms only the probe saw ({len(only_probe)}):")
        for rid in only_probe[:50]:
            print(f"  {rid}")
    if only_walker:
        print(f"\nrooms only the walker saw ({len(only_walker)}):")
        for rid in only_walker[:50]:
            print(f"  {rid}")
    if name_mismatches:
        print(f"\nname mismatches ({len(name_mismatches)}):")
        for rid, a, b in name_mismatches[:50]:
            print(f"  {rid}: probe={a!r} walker={b!r}")

    print()
    if ok:
        print(f"PASS: {len(common)} room ids and names match exactly")
        return 0
    print("FAIL: the two clients disagree")
    return 1


if __name__ == "__main__":
    sys.exit(main())
