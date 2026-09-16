#!/usr/bin/env bash
# Produce a scrubbed copy of this repo for publication, leaving the working
# tree untouched. Reads tools/public-scrub.map (gitignored, local-only).
#
#   tools/publish-public.sh                 # build + verify, no push
#   tools/publish-public.sh <remote-url>    # build, verify, then push
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAP="$REPO_DIR/tools/public-scrub.map"
REMOTE="${1:-}"

if [ ! -f "$MAP" ]; then
  echo "error: $MAP not found — it holds the substitutions and is gitignored" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "==> cloning $REPO_DIR"
git clone --quiet --no-local "$REPO_DIR" "$WORK/repo"
cd "$WORK/repo"

# A blob callback, not --replace-text: the latter skips binary blobs, so a
# captured fixture would keep its server URL in every commit. filter-repo takes
# the callback as an inline body, and passing a file path fails silently.
BODY=$(python3 - "$MAP" <<'PY'
import sys

pairs = []
patterns = []
with open(sys.argv[1], "rb") as handle:
    for raw in handle:
        line = raw.rstrip(b"\n")
        if line.startswith(b"literal:"):
            old, _, new = line[len(b"literal:"):].partition(b"==>")
            pairs.append((old, new))
        elif line.startswith(b"regex:"):
            old, _, new = line[len(b"regex:"):].partition(b"==>")
            patterns.append((old, new))

out = ["import re"]
for old, new in pairs:
    out.append("blob.data = blob.data.replace(%r, %r)" % (old, new))
for old, new in patterns:
    out.append("blob.data = re.sub(%r, %r, blob.data)" % (old, new))
sys.stdout.write("\n".join(out))
PY
)

echo "==> rewriting history ($(git rev-list --count HEAD) commits) with a blob callback"
git filter-repo --quiet --blob-callback "$BODY" --force

echo "==> verifying the published tree"
PATTERNS='example-user|example-user|library\.lan|192\.168\.|$HOME'
tree_hits=$(git grep -alE "$PATTERNS" HEAD -- . 2>/dev/null || true)
if [ -n "$tree_hits" ]; then
  echo "FAILED: tree still matches:" >&2
  echo "$tree_hits" >&2
  exit 1
fi
echo "    tree clean"

echo "==> verifying every commit's blobs (binaries included)"
survivors=""
for commit in $(git rev-list --all); do
  hits=$(git grep -alE "$PATTERNS" "$commit" -- . 2>/dev/null || true)
  if [ -n "$hits" ]; then
    survivors="$commit:$hits"
    break
  fi
done
if [ -n "$survivors" ]; then
  echo "FAILED: history still matches:" >&2
  echo "$survivors" >&2
  exit 1
fi
echo "    history clean across $(git rev-list --count HEAD) commits"

if [ -n "$REMOTE" ]; then
  echo "==> pushing to $REMOTE"
  git remote add origin "$REMOTE"
  git push --quiet -u origin HEAD:main
  echo "    pushed"
else
  echo "==> scrubbed copy ready at $WORK/repo (no remote given — nothing pushed)"
fi
