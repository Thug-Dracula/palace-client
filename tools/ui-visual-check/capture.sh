#!/usr/bin/env bash
# Convenience wrapper: run the headless visual harness from anywhere.
# Usage: tools/ui-visual-check/capture.sh [capture.py args]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$here/capture.py" "$@"
