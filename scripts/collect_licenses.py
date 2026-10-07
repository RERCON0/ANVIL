"""Collect the licence notices of the Rust crates the release binaries are built from.

The list is the normal-dependency graph of the root crate. It also holds compile-time
crates (proc macros such as serde_derive and the syn/quote stack behind them) that are
not linked into the executables, and Cargo metadata cannot tell reliably which ones.
It is wider than needed on purpose: an extra notice is harmless, a missing one is not.

Usage: python -B scripts/collect_licenses.py <target-triple> <output-file>
"""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
LICENSE_FILES = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "UNLICENSE", "NOTICE")


def linked_crates(target):
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target],
        cwd=ROOT, encoding="utf-8"))
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    root = metadata["resolve"]["root"]
    seen, pending = set(), [root]
    while pending:
        crate = pending.pop()
        if crate not in seen:
            seen.add(crate)
            # Normal edges only: dev- and build-dependencies are not shipped.
            pending += [dep["pkg"] for dep in nodes[crate]["deps"] if any(kind["kind"] is None for kind in dep["dep_kinds"])]
    return sorted((packages[crate] for crate in seen - {root}), key=lambda package: (package["name"], package["version"]))


def render(target):
    texts, without_text = {}, []
    for package in linked_crates(target):
        license_id = package["license"] or "no SPDX expression"
        label = f"{package['name']} {package['version']} ({license_id})"
        folder = Path(package["manifest_path"]).parent
        files = sorted(path for path in folder.iterdir() if path.is_file() and path.name.upper().startswith(LICENSE_FILES))
        if not files:
            without_text.append(f"{label} {package['repository'] or ''}".rstrip())
        for path in files:
            text = path.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n").strip("\n").rstrip()
            texts.setdefault(text, []).append(f"{label} - {path.name}")
    out = [f"Rust crates in the normal dependency graph of the ANVIL release build for {target}, with the licence",
           "files of their published sources. The list also covers compile-time crates (proc macros and the",
           "crates they use) that are not linked into the executables."]
    for text, users in sorted(texts.items(), key=lambda item: item[1][0]):
        out += ["", "=" * 78, "Used by:", *(f"  {user}" for user in users), "", text]
    if without_text:
        out += ["", "=" * 78,
                "Published without a licence file (see the SPDX expression and the repository):",
                *(f"  {crate}" for crate in without_text)]
    return "\n".join(out) + "\n"


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    Path(sys.argv[2]).write_text(render(sys.argv[1]), encoding="utf-8", newline="\n")
