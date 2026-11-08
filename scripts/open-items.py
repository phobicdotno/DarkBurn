#!/usr/bin/env python3
"""List checklist items that are not fully done.

Usage: python scripts/open-items.py [PREFIX ...] [--open] [--partial]
  PREFIX     only items whose ID prefix matches, for example SHAPE NODE TOOLS
  --open     only items with [ ]
  --partial  only items with [~]
With no prefix, every group is listed.
"""
import re, sys, pathlib

args = sys.argv[1:]
want_open = "--open" in args
want_partial = "--partial" in args
prefixes = {a for a in args if not a.startswith("--")}
text = pathlib.Path(__file__).resolve().parent.parent.joinpath("docs", "PARITY-CHECKLIST.md").read_text(encoding="utf-8")
count = 0
for m in re.finditer(r"^- \[(.)\] ([A-Z0-9]+)-(\d+) (.*)$", text, re.M):
    mark, group, num, title = m.groups()
    if mark == "x":
        continue
    if prefixes and group not in prefixes:
        continue
    if want_open and mark != " ":
        continue
    if want_partial and mark != "~":
        continue
    print(f"{group}-{num} [{mark}] {title}")
    count += 1
print(f"-- {count} items", file=sys.stderr)
