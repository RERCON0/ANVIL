"""Regression test for gen_seti_icons.py: the generated font must be reproducible.

CI installs the hash-pinned fontTools wheel from scripts/requirements-icons.txt.
Run it after touching the generator or vendored font:

    python -B -m unittest discover -s scripts -p "test_*.py"
"""
import shutil
import hashlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

try:
    import fontTools  # noqa: F401
except ImportError:
    fontTools = None


@unittest.skipIf(fontTools is None, "fontTools is not installed; install scripts/requirements-icons.txt")
class GenSetiIcons(unittest.TestCase):
    def test_vendored_inputs_match_reviewed_checksums(self):
        vendor = ROOT / "vendor" / "seti"
        entries = [line.split("  ", 1) for line in (vendor / "checksums.sha256").read_text().splitlines()]
        self.assertEqual({name for _, name in entries}, {"seti.woff", "setiIconMap.ts", "seti-LICENSE.txt"})
        self.assertEqual(len(entries), 3)
        for expected, name in entries:
            self.assertEqual(hashlib.sha256((vendor / name).read_bytes()).hexdigest(), expected)

    def generate(self, work):
        subprocess.run([sys.executable, "-B", str(work / "scripts" / "gen_seti_icons.py")], check=True, capture_output=True)

    def test_regeneration_reproduces_the_committed_font(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "scripts").mkdir()
            (work / "src").mkdir()
            shutil.copy(ROOT / "scripts" / "gen_seti_icons.py", work / "scripts")
            shutil.copytree(ROOT / "vendor" / "seti", work / "vendor" / "seti")

            self.generate(work)
            first = (work / "fonts" / "seti.ttf").read_bytes()
            self.generate(work)
            self.assertEqual((work / "fonts" / "seti.ttf").read_bytes(), first, "the font must not carry a build time")

            self.assertEqual(first, (ROOT / "fonts" / "seti.ttf").read_bytes(),
                             "fonts/seti.ttf differs: run python scripts/gen_seti_icons.py")
            self.assertEqual((work / "src" / "file_icons.rs").read_bytes(),
                             (ROOT / "src" / "file_icons.rs").read_bytes(),
                             "src/file_icons.rs differs: run python scripts/gen_seti_icons.py")


if __name__ == "__main__":
    unittest.main()
