#!/usr/bin/env python3
"""Screenshot each editor tool tab (thin wrapper around capture.py --tabs).

Kept as a separate entry point because earlier sessions used it by name.
All the real work (headless Chromium, Tauri stub, safety rules) lives in
capture.py -- see that file and README.md.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from capture import main  # noqa: E402

if __name__ == "__main__":
    sys.argv = [sys.argv[0], "--tabs", *sys.argv[1:]]
    sys.exit(main())
