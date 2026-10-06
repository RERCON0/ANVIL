"""Verify the reviewed x64 ConPTY runtime before compiling or packaging."""
import hashlib
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
BUNDLE = ROOT / "vendor" / "conpty"
EXPECTED_FILES = {"x64/conpty.dll", "x64/OpenConsole.exe"}


def verify():
    entries = {}
    for line in (BUNDLE / "checksums.sha256").read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (x64/[^/]+)", line)
        if not match or match[2] not in EXPECTED_FILES or match[2] in entries:
            raise ValueError("Invalid ConPTY checksum manifest")
        entries[match[2]] = match[1]
    if set(entries) != EXPECTED_FILES:
        raise ValueError("Incomplete ConPTY checksum manifest")
    for name, expected in entries.items():
        with (BUNDLE / name).open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
        if actual != expected:
            raise ValueError(f"ConPTY SHA-256 mismatch: {name}")
    print("Verified vendored ConPTY x64 runtime")


if __name__ == "__main__":
    verify()
