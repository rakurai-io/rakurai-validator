#!/usr/bin/env python3
"""Convert docs between Redocly (Markdoc) and GitHub markdown flavors.

Two things differ between the flavors and both are handled here:

  1. Callouts   {% admonition type="warning" name="T" %}  <->  > [!WARNING]
                body                                            > **T**
                {% /admonition %}                               >
                                                                > body

  2. Anchors    Redocly slugs keep '.' and '/'; GitHub drops them, so every
                internal link fragment is re-slugged for the target flavor.

Usage:
  python3 convert_flavor.py --to github
  python3 convert_flavor.py --to redocly
  python3 convert_flavor.py --to github --check          # report, write nothing
  python3 convert_flavor.py --to github --out-dir ../gh  # write into a tree
"""
import argparse
import os
import re
import shutil
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import _common  # noqa: E402
from _common import (  # noqa: E402
    ADMON_OPEN, ADMON_CLOSE, ALERT_OPEN, TYPE_TO_ALERT, ALERT_TO_TYPE,
    md_files, rel, resolve_link, headings, strip_number,
)

BOLD_ONLY = re.compile(r"^\*\*(.+)\*\*$")
LINK_FRAG = re.compile(r"\]\((?!https?:|mailto:)([^)#\s]*)#([^)\s]+)\)")


# --------------------------------------------------------------------------- #
# callouts
# --------------------------------------------------------------------------- #
def admonitions_to_alerts(lines):
    """Redocly -> GitHub."""
    out, i, n = [], 0, 0
    while i < len(lines):
        m = ADMON_OPEN.match(lines[i])
        if not m:
            out.append(lines[i])
            i += 1
            continue
        kind = TYPE_TO_ALERT.get(m.group("type"))
        if kind is None:
            raise ValueError(f"unknown admonition type {m.group('type')!r}")
        body, i = [], i + 1
        while i < len(lines) and not ADMON_CLOSE.match(lines[i]):
            body.append(lines[i])
            i += 1
        if i >= len(lines):
            raise ValueError("unclosed {% admonition %}")
        i += 1  # consume the close tag

        block = [f"> [!{kind}]"]
        name = m.group("name")
        if name:
            block.append(f"> **{name}**")
            if body and body[0].strip():
                block.append(">")
        block += [(f"> {ln}" if ln.strip() else ">") for ln in body]
        while len(block) > 1 and block[-1] == ">":
            block.pop()
        out += block
        n += 1
    return out, n


def alerts_to_admonitions(lines):
    """GitHub -> Redocly."""
    out, i, n = [], 0, 0
    while i < len(lines):
        m = ALERT_OPEN.match(lines[i])
        if not m:
            out.append(lines[i])
            i += 1
            continue
        atype = ALERT_TO_TYPE.get(m.group("kind"))
        if atype is None:
            raise ValueError(f"unknown GitHub alert kind {m.group('kind')!r}")
        body, i = [], i + 1
        while i < len(lines) and lines[i].startswith(">"):
            body.append(re.sub(r"^>[ ]?", "", lines[i]))
            i += 1

        name = ""
        if body:
            bm = BOLD_ONLY.match(body[0].strip())
            if bm:
                name = bm.group(1)
                body = body[1:]
                while body and not body[0].strip():
                    body = body[1:]
        while body and not body[-1].strip():
            body.pop()

        head = f'{{% admonition type="{atype}"'
        head += f' name="{name}" %}}' if name else " %}"
        out += [head] + body + ["{% /admonition %}"]
        n += 1
    return out, n


# --------------------------------------------------------------------------- #
# anchors
# --------------------------------------------------------------------------- #
def reslug(text, target, head_index, path, problems):
    """Rewrite every internal link fragment into `target` flavor slugs."""
    count = [0]

    def repl(m):
        link, frag = m.group(1), m.group(2)
        tgt = resolve_link(path, link)
        hs = head_index.get(tgt) if tgt else None
        if hs is None:
            hs = headings(tgt) if tgt else None
        if hs is None:
            problems.append(f"{rel(path)}: link to missing file {link}")
            return m.group(0)
        bare = strip_number(frag)
        for h in hs:
            if frag in (h["redocly"], h["github"]) or bare in (h["bare_redocly"], h["bare_github"]):
                new = h[target]
                if new != frag:
                    count[0] += 1
                return f"]({link}#{new})"
        problems.append(f"{rel(path)}: unresolved '#{frag}' -> {rel(tgt)}")
        return m.group(0)

    return LINK_FRAG.sub(repl, text), count[0]


# --------------------------------------------------------------------------- #
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--to", required=True, choices=["github", "redocly"])
    ap.add_argument("--check", action="store_true", help="report only, write nothing")
    ap.add_argument("--out-dir", help="write into this tree instead of in place")
    ap.add_argument("--root", help="convert this tree instead of the repo "
                                   "(e.g. the rakurai-docs checkout in CI)")
    args = ap.parse_args()

    if args.root:
        _common.set_root(args.root)
    files = md_files()
    if not files:
        print(f"no markdown found under {_common.ROOT}", file=sys.stderr)
        return 1
    head_index = {f: headings(f) for f in files}
    problems, stats = [], []

    for f in files:
        src = open(f, encoding="utf-8").read()
        nl = src.endswith("\n")
        lines = src.split("\n")
        if nl:
            lines.pop()

        try:
            if args.to == "github":
                lines, n = admonitions_to_alerts(lines)
            else:
                lines, n = alerts_to_admonitions(lines)
        except ValueError as e:
            problems.append(f"{rel(f)}: {e}")
            continue

        body = "\n".join(lines) + ("\n" if nl else "")
        body, a = reslug(body, args.to, head_index, f, problems)

        if n or a:
            stats.append((rel(f), n, a))
        if args.check:
            continue

        dest = f if not args.out_dir else os.path.join(
            os.path.abspath(args.out_dir), rel(f))
        if dest != f:
            os.makedirs(os.path.dirname(dest), exist_ok=True)
        if body != src or dest != f:
            open(dest, "w", encoding="utf-8").write(body)

    verb = "would convert" if args.check else "converted"
    print(f"=== {verb} to {args.to} ===")
    for p, n, a in stats:
        print(f"  {n:3d} callouts  {a:3d} anchors   {p}")
    print(f"  files touched: {len(stats)}  "
          f"callouts: {sum(s[1] for s in stats)}  anchors: {sum(s[2] for s in stats)}")

    if problems:
        print("\n=== problems ===")
        for p in problems:
            print("  " + p)
        return 1
    print("\nno problems")
    return 0


if __name__ == "__main__":
    sys.exit(main())
