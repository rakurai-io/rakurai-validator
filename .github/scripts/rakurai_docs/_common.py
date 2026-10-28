"""Shared helpers for the Rakurai docs tooling.

Repo root is derived from this file's location:
  <root>/.github/scripts/rakurai_docs/_common.py
"""
import os
import re
import collections

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))


def set_root(path):
    """Point the helpers at another tree, e.g. the rakurai-docs checkout in CI.

    Callers that imported ROOT by name must re-read `_common.ROOT` afterwards.
    """
    global ROOT
    ROOT = os.path.abspath(path)
    return ROOT

# Everything docs-sync.yml publishes. Program/CLI docs live under
# rakurai_docs/rakurai_programs/ and are edited there directly — no submodule sync.
PUBLISHED = [
    "README.md",
    "rakurai_docs",
    "spark-geyser/README.md",
    "block-rewards-distributor/block_reward_distribution.md",
]

# Redocly admonition type <-> GitHub alert kind.
TYPE_TO_ALERT = {
    "info": "NOTE",
    "success": "TIP",
    "warning": "WARNING",
    "danger": "CAUTION",
}
ALERT_TO_TYPE = {v: k for k, v in TYPE_TO_ALERT.items()}
ALERT_TO_TYPE["IMPORTANT"] = "warning"  # GitHub-only kind we never emit

ADMON_OPEN = re.compile(
    r'^\{%\s*admonition\s+type="(?P<type>\w+)"(?:\s+name="(?P<name>[^"]*)")?\s*%\}\s*$'
)
ADMON_CLOSE = re.compile(r"^\{%\s*/admonition\s*%\}\s*$")
ALERT_OPEN = re.compile(r"^>\s*\[!(?P<kind>[A-Z]+)\]\s*$")

_LEADING_NUM = re.compile(r"^\d+(?:\.\d+)*\.?[-\s]+")


def md_files():
    """Absolute paths of every markdown file the docs pipeline touches."""
    out = []
    for rel_path in PUBLISHED:
        p = os.path.join(ROOT, rel_path)
        if os.path.isfile(p):
            out.append(p)
        elif os.path.isdir(p):
            for d, _, fs in os.walk(p):
                out += [os.path.join(d, f) for f in fs if f.endswith(".md")]
    return sorted(set(out))


def rel(path):
    return os.path.relpath(path, ROOT)


def resolve_link(path, link):
    """Absolute target of a relative link, or None."""
    base = os.path.dirname(path)
    tgt = os.path.normpath(os.path.join(base, link)) if link else path
    return tgt if os.path.exists(tgt) else None


def clean_heading(text):
    """Strip inline markdown so slugging sees the rendered text."""
    text = re.sub(r"`([^`]*)`", r"\1", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"\*\*([^*]*)\*\*", r"\1", text)
    text = re.sub(r"\*([^*]*)\*", r"\1", text)
    return text.strip()


def _slug(text, keep):
    t = clean_heading(text).lower()
    t = "".join(c for c in t if c.isalnum() or c in keep)
    return t.replace(" ", "-")


def redocly_slug(text):
    """Redocly (Markdoc) keeps '.' and '/'. Validated against docs.rakurai.io."""
    return _slug(text, " ./-_")


def github_slug(text):
    """GitHub drops '.' and '/'."""
    return _slug(text, " -_")


def strip_number(slug):
    """'3.2.-epoch-flow' -> 'epoch-flow', so renumbering does not break links."""
    return _LEADING_NUM.sub("", slug)


def headings(path):
    """Ordered heading records with both slug flavors and duplicate suffixes."""
    out, rc, gc = [], collections.Counter(), collections.Counter()
    fence = False
    for line in open(path, encoding="utf-8"):
        stripped = line.lstrip()
        if stripped.startswith("```"):
            fence = not fence
            continue
        if fence:
            continue
        # A '#' inside a GitHub alert body is still a heading to neither flavor.
        m = re.match(r"^(#{1,6})\s+(.*?)\s*$", line)
        if not m:
            continue
        text = m.group(2)
        r, g = redocly_slug(text), github_slug(text)
        nr, ng = rc[r], gc[g]
        rc[r] += 1
        gc[g] += 1
        out.append({
            "level": len(m.group(1)),
            "text": text,
            "redocly": r if not nr else f"{r}-{nr}",
            "github": g if not ng else f"{g}-{ng}",
            "bare_redocly": strip_number(r),
            "bare_github": strip_number(g),
        })
    return out


def detect_flavor(path):
    """'redocly', 'github', or 'plain' for a single file."""
    src = open(path, encoding="utf-8").read()
    if ADMON_OPEN.search(src) or "{% admonition" in src:
        return "redocly"
    if re.search(r"^>\s*\[!\w+\]", src, re.M):
        return "github"
    return "plain"
