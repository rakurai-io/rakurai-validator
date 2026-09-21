#!/usr/bin/env python3
"""Lint the Rakurai docs: flavor purity, callouts, heading numbers, links.

Usage:
  python3 verify_docs.py                 # auto-detect the dominant flavor
  python3 verify_docs.py --flavor redocly
  python3 verify_docs.py --flavor github
"""
import argparse
import collections
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import _common  # noqa: E402
from _common import (  # noqa: E402
    ADMON_OPEN, ADMON_CLOSE, ALERT_OPEN, TYPE_TO_ALERT,
    md_files, rel, resolve_link, headings, clean_heading, strip_number, detect_flavor,
)

# Unnumbered top-level sections, and anything nested under them.
EXEMPT_H2 = ("appendix", "related")
NUMBERED = re.compile(r"^(\d+(?:\.\d+)*)\.")
LINK_ANY = re.compile(r"\]\((?!https?:|mailto:)([^)\s]+)\)")

fails = 0
_section_fails = 0


def section(title):
    global _section_fails
    _section_fails = 0
    print(f"\n=== {title} ===")


def bad(msg):
    global fails, _section_fails
    fails += 1
    _section_fails += 1
    print(f"  {msg}")


def ok(note="ok"):
    if not _section_fails:
        print(f"  {note}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--flavor", choices=["redocly", "github"])
    ap.add_argument("--root", help="lint this tree instead of the repo")
    args = ap.parse_args()

    if args.root:
        _common.set_root(args.root)
    files = md_files()
    if not files:
        print(f"no markdown found under {_common.ROOT}", file=sys.stderr)
        return 1
    detected = collections.Counter(detect_flavor(f) for f in files)
    flavor = args.flavor or (detected.most_common(1)[0][0]
                             if detected.most_common(1)[0][0] != "plain" else "redocly")
    print(f"flavor: {flavor}  (detected: {dict(detected)})")

    # ---------------------------------------------------------------- flavor
    section("flavor purity")
    for f in files:
        src = open(f, encoding="utf-8").read()
        has_admon = "{% admonition" in src
        has_alert = bool(re.search(r"^>\s*\[!\w+\]", src, re.M))
        if flavor == "redocly" and has_alert:
            bad(f"GitHub alert in redocly tree: {rel(f)}")
        if flavor == "github" and has_admon:
            bad(f"Markdoc admonition in github tree: {rel(f)}")
        if re.search(r"^:::", src, re.M):
            bad(f"Docusaurus ':::' block (neither flavor renders it): {rel(f)}")
        if re.search(r"<a\s+id=", src):
            bad(f"raw <a id> anchor (Redocly strips the id): {rel(f)}")
    ok()

    # -------------------------------------------------------------- callouts
    section("callout syntax")
    kinds = collections.Counter()
    for f in files:
        lines = open(f, encoding="utf-8").read().split("\n")
        depth, fences = 0, 0
        for n, line in enumerate(lines, 1):
            if line.lstrip().startswith("```"):
                fences += 1
            if flavor == "redocly":
                if "{% admonition" in line:
                    m = ADMON_OPEN.match(line)
                    if not m:
                        bad(f"malformed open {rel(f)}:{n}: {line[:70]}")
                        continue
                    if m.group("type") not in TYPE_TO_ALERT:
                        bad(f"unapproved type {m.group('type')!r} {rel(f)}:{n}")
                    kinds[m.group("type")] += 1
                    depth += 1
                    if depth > 1:
                        bad(f"nested callout {rel(f)}:{n}")
                    if n > 1 and lines[n - 2].strip() and not lines[n - 2].startswith("#"):
                        bad(f"missing blank line before callout {rel(f)}:{n}")
                elif ADMON_CLOSE.match(line):
                    depth -= 1
                    if n < len(lines) and lines[n].strip():
                        bad(f"missing blank line after callout {rel(f)}:{n}")
                elif "{% /admonition" in line:
                    bad(f"malformed close {rel(f)}:{n}: {line[:70]}")
            else:
                m = ALERT_OPEN.match(line)
                if m:
                    kinds[m.group("kind")] += 1
                    if n > 1 and lines[n - 2].strip() and not lines[n - 2].startswith("#"):
                        bad(f"missing blank line before alert {rel(f)}:{n}")
                    if n >= len(lines) or not lines[n].startswith(">"):
                        bad(f"empty alert body {rel(f)}:{n}")
        if depth:
            bad(f"unbalanced callouts (depth {depth}): {rel(f)}")
        if fences % 2:
            bad(f"odd number of code fences ({fences}): {rel(f)}")
    ok("  ".join(f"{k}={v}" for k, v in sorted(kinds.items())) or "none")

    # ------------------------------------------------------- heading numbers
    section("heading numbering")
    for f in files:
        exempt_from = 99
        for h in headings(f):
            lvl, text = h["level"], clean_heading(h["text"])
            if lvl == 1:
                continue
            if lvl <= exempt_from:
                exempt_from = 99
            if lvl > exempt_from:
                continue
            if lvl == 2 and text.lower().startswith(EXEMPT_H2):
                exempt_from = lvl
                continue
            m = NUMBERED.match(text)
            if not m:
                bad(f"unnumbered H{lvl} in {rel(f)}: {text}")
                continue
            depth = m.group(1).count(".") + 1
            if depth != lvl - 1:
                bad(f"H{lvl} numbered {m.group(1)} (want depth {lvl - 1}) in {rel(f)}: {text}")
    ok()

    # ----------------------------------------------------- fragile anchors
    section("duplicate heading slugs")
    key = "redocly" if flavor == "redocly" else "github"
    for f in files:
        slugs = [h[key] for h in headings(f)]
        dups = sorted({s.rsplit("-", 1)[0] for s in slugs
                       if re.search(r"-\d+$", s) and s.rsplit("-", 1)[0] in slugs})
        if dups:
            bad(f"{rel(f)}: repeated heading text -> suffixed anchors {dups}")
    ok()

    # ------------------------------------------------------------- links
    section("links and anchors")
    index = {f: headings(f) for f in files}
    for f in files:
        src = open(f, encoding="utf-8").read()
        for target in LINK_ANY.findall(src):
            link, _, frag = target.partition("#")
            tgt = resolve_link(f, link)
            if tgt is None:
                bad(f"{rel(f)} -> missing {link}")
                continue
            if not frag or not tgt.endswith(".md"):
                continue
            hs = index.get(tgt) or headings(tgt)
            if frag in [h[key] for h in hs]:
                continue
            alt = "github" if key == "redocly" else "redocly"
            if frag in [h[alt] for h in hs]:
                bad(f"{rel(f)} -> '#{frag}' is a {alt} slug, expected {key}")
            elif strip_number(frag) in [h[f"bare_{key}"] for h in hs]:
                bad(f"{rel(f)} -> '#{frag}' has a stale section number")
            else:
                bad(f"{rel(f)} -> unresolved '#{frag}' in {rel(tgt)}")
    ok()

    print(f"\n{'PASS' if not fails else f'FAIL: {fails} issue(s)'}")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
