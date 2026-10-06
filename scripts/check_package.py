"""Check the unsigned CI package without extracting or running its payload."""
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
import zipfile

from check_conpty import ROOT, verify as verify_conpty

ASSETS = {
    "conpty.dll": "vendor/conpty/x64/conpty.dll",
    "OpenConsole.exe": "vendor/conpty/x64/OpenConsole.exe",
    "LICENSE": "LICENSE",
    "LICENSES/conpty-MIT.txt": "vendor/conpty/LICENSE",
    "LICENSES/CascadiaMono-OFL-notice.txt": "fonts/OFL-notice.txt",
    "LICENSES/OFL-1.1.txt": "fonts/OFL-1.1.txt",
    "LICENSES/SetiUI-MIT.txt": "fonts/seti-LICENSE.txt",
}
for name in ("Hack-Regular.txt", "Ubuntu-Light-UFL.txt", "NotoEmoji-OFL.txt", "emoji-icon-font-MIT.txt"):
    ASSETS[f"LICENSES/egui-default-fonts/{name}"] = f"LICENSES/egui-default-fonts/{name}"
EXPECTED = set(ASSETS) | {"anvil.exe", "anvil-claude-status.exe", "SOURCE.txt", "BUILD.json"}


def check_pe(data, subsystem, name):
    if len(data) < 64 or data[:2] != b"MZ":
        raise ValueError(f"Invalid executable: {name}")
    offset = struct.unpack_from("<I", data, 0x3C)[0]
    if offset + 96 > len(data) or data[offset:offset + 4] != b"PE\0\0":
        raise ValueError(f"Invalid PE header: {name}")
    machine = struct.unpack_from("<H", data, offset + 4)[0]
    magic = struct.unpack_from("<H", data, offset + 24)[0]
    actual_subsystem, flags = struct.unpack_from("<HH", data, offset + 24 + 68)
    if machine != 0x8664 or magic != 0x20B or actual_subsystem != subsystem:
        raise ValueError(f"Wrong x64 architecture or subsystem: {name}")
    if flags & 0x160 != 0x160:
        raise ValueError(f"ASLR, high-entropy ASLR or DEP missing: {name}")


def check(package):
    verify_conpty()
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    with zipfile.ZipFile(package) as archive:
        files = [entry for entry in archive.infolist() if not entry.is_dir()]
        if len(files) != len(EXPECTED) or {entry.filename for entry in files} != EXPECTED:
            raise ValueError("Unexpected, missing or duplicate package members")
        if any(entry.file_size > 32 * 1024 * 1024 or entry.flag_bits & 1 for entry in files):
            raise ValueError("Oversized or encrypted package member")
        payload = {entry.filename: archive.read(entry) for entry in files}
    metadata = json.loads(payload["BUILD.json"].decode("utf-8-sig"))
    version = re.search(r'^version\s*=\s*"([^"]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.M)[1]
    if (metadata.get("schema") != 1 or metadata.get("version") != version
            or metadata.get("source_commit") != commit or metadata.get("features") != ["codex"]
            or metadata.get("target") != "x86_64-pc-windows-msvc"):
        raise ValueError("Build metadata differs from the Codex-enabled CI revision")
    if set(metadata["files"]) != EXPECTED - {"BUILD.json"}:
        raise ValueError("Incomplete payload hashes")
    for name, expected in metadata["files"].items():
        if expected != {"sha256": hashlib.sha256(payload[name]).hexdigest(), "size": len(payload[name])}:
            raise ValueError(f"Payload hash or size mismatch: {name}")
    for name, source in ASSETS.items():
        if payload[name] != (ROOT / source).read_bytes():
            raise ValueError(f"Bundled runtime or licence differs from reviewed source: {name}")
    source = payload["SOURCE.txt"].decode("utf-8-sig").splitlines()
    if f"Commit: {commit}" not in source or "Features: codex" not in source:
        raise ValueError("Source notice differs from CI revision")
    check_pe(payload["anvil.exe"], 2, "anvil.exe")
    check_pe(payload["anvil-claude-status.exe"], 3, "anvil-claude-status.exe")
    checksum = hashlib.sha256(package.read_bytes()).hexdigest()
    declared = (package.parent / "SHA256SUMS.txt").read_text(encoding="ascii").strip()
    if declared != f"{checksum}  {package.name}":
        raise ValueError("Archive SHA-256 mismatch")
    print(f"Verified unsigned ANVIL {version} / {commit} / Codex enabled")


if __name__ == "__main__":
    path = Path(sys.argv[1])
    if path.is_dir():
        candidates = list(path.glob("anvil-*-x64.zip"))
        if len(candidates) != 1:
            raise ValueError("Expected exactly one CI package")
        path = candidates[0]
    check(path)
