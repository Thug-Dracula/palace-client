#!/usr/bin/env bash
# Produce a scrubbed copy of this repo for publication, leaving the working
# tree untouched. Reads tools/public-scrub.map (gitignored, local-only).
#
#   tools/publish-public.sh                        # build + verify, no push
#   tools/publish-public.sh <remote-url>           # build, verify, then push
#   tools/publish-public.sh <remote-url> --force   # replace the public history
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAP="$REPO_DIR/tools/public-scrub.map"
REMOTE="${1:-}"
FORCE="${2:-}"

if [ ! -f "$MAP" ]; then
  echo "error: $MAP not found — it holds the substitutions and is gitignored" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "==> cloning $REPO_DIR"
git clone --quiet --no-local "$REPO_DIR" "$WORK/repo"
cd "$WORK/repo"

# A clone inherits its source as `origin`, which points at the local repo —
# so a later `remote add origin <url>` fails and the push would silently land
# back on this disk instead of GitHub.
git remote remove origin

# The map becomes filter-repo callbacks. A blob callback (not --replace-text)
# so binary blobs are scrubbed too, plus a message callback so a secret never
# survives in a commit message. filter-repo takes each callback as an inline
# body; passing a file path fails silently.
GEN_PY='
import sys
mode = sys.argv[2]
pairs, patterns, excludes = [], [], []
with open(sys.argv[1], "rb") as handle:
    for raw in handle:
        line = raw.rstrip(b"\n")
        if line.startswith(b"exclude:"):
            excludes.append(line[len(b"exclude:"):])
        elif line.startswith(b"literal:"):
            old, _, new = line[len(b"literal:"):].partition(b"==>")
            pairs.append((old, new))
        elif line.startswith(b"regex:"):
            old, _, new = line[len(b"regex:"):].partition(b"==>")
            patterns.append((old, new))
if mode == "blob":
    out = ["import re"]
    for old, new in pairs:
        out.append("blob.data = blob.data.replace(%r, %r)" % (old, new))
    for old, new in patterns:
        out.append("blob.data = re.sub(%r, %r, blob.data)" % (old, new))
elif mode == "message":
    out = ["import re"]
    for old, new in pairs:
        out.append("message = message.replace(%r, %r)" % (old, new))
    for old, new in patterns:
        out.append("message = re.sub(%r, %r, message)" % (old, new))
    out.append("return message")
else:
    out = [e.decode() for e in excludes]
sys.stdout.write("\n".join(out))
'

BODY_FILE="$WORK/blob-callback.py"
MSG_FILE="$WORK/message-callback.py"
python3 -c "$GEN_PY" "$MAP" blob > "$BODY_FILE"
python3 -c "$GEN_PY" "$MAP" message > "$MSG_FILE"
python3 -c "$GEN_PY" "$MAP" excludes > "$WORK/excludes.txt"

FILTER_ARGS=()
# `read` returns non-zero at EOF without a trailing newline, which would
# silently drop the final exclude; keep the last line when it is non-empty.
while IFS= read -r pattern || [ -n "$pattern" ]; do
  [ -n "$pattern" ] && FILTER_ARGS+=(--path-glob "$pattern")
done < "$WORK/excludes.txt"
INVERT=()
if [ "${#FILTER_ARGS[@]}" -gt 0 ]; then
  INVERT=(--invert-paths)
  echo "==> excluding local-only paths: ${FILTER_ARGS[*]}"
fi

BEFORE=$(git rev-list --count HEAD)
ORIG_TIP=$(git rev-parse HEAD)
echo "==> rewriting history ($BEFORE commits)"
git filter-repo --quiet \
  --blob-callback "$(cat "$BODY_FILE")" \
  --message-callback "$(cat "$MSG_FILE")" \
  ${FILTER_ARGS[@]+"${FILTER_ARGS[@]}"} ${INVERT[@]+"${INVERT[@]}"} --force
NEW_TIP=$(git rev-parse HEAD)
if [ "$ORIG_TIP" = "$NEW_TIP" ]; then
  echo "FAILED: history unchanged — the filter did not run" >&2
  exit 1
fi

echo "==> verifying (the canonical repository URL is the one permitted exception)"
PATTERNS='example-user|example-user|example-user|library\.lan|192\.168\.|$HOME|input-tool|input-tool|input-tool|input-tool|agent-runner|Note:|/tmp/work|\$CORPUS|$CORPUS|$MEDIA/|$MEDIA/|$HOME/ProgramFiles/|colosseum\.thug|PC[0-9]{4}'
ALLOWED='repository = "https://github.com/Thug-Dracula/palace-client"'

leaks=$(git grep -nE "$PATTERNS" HEAD -- . 2>/dev/null | grep -vF "$ALLOWED" || true)
if [ -n "$leaks" ]; then
  echo "FAILED: tree still matches:" >&2
  echo "$leaks" >&2
  exit 1
fi
echo "    tree clean apart from the repository URL"

echo "==> verifying every commit's messages"
leaks=$(git log --all --format='%H %s%n%b' | grep -nE "$PATTERNS" || true)
if [ -n "$leaks" ]; then
  echo "FAILED: a commit message still matches:" >&2
  echo "$leaks" >&2
  exit 1
fi
echo "    commit messages clean"

echo "==> verifying every commit's blobs (binaries included)"
leaks=""
for commit in $(git rev-list --all); do
  hits=$(git grep -nE "$PATTERNS" "$commit" -- . 2>/dev/null | grep -vF "$ALLOWED" || true)
  if [ -n "$hits" ]; then
    leaks="$commit $hits"
    break
  fi
done
if [ -n "$leaks" ]; then
  echo "FAILED: history still matches:" >&2
  echo "$leaks" >&2
  exit 1
fi
echo "    history clean across $(git rev-list --count HEAD) commits"

if [ -n "$REMOTE" ]; then
  echo "==> pushing to $REMOTE"
  git remote add origin "$REMOTE"
  if [ "$FORCE" = "--force" ]; then
    echo "    replacing public history (force)"
    git push --quiet --force -u origin HEAD:main
  else
    git push --quiet -u origin HEAD:main
  fi
  echo "    pushed"
else
  echo "==> scrubbed copy ready at $WORK/repo (no remote given — nothing pushed)"
fi
