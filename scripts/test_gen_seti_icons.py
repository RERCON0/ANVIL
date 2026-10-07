"""Regression test for gen_seti_icons.py: the generated font must be reproducible.

It needs fontTools, which CI does not install (everything installed there is pinned and
hash-checked), so GitHub Actions only reports this test as skipped. It really runs on a
developer machine with `pip install "fontTools==4.63.0"`; run it after touching the
generator or vendored font:

    python -B -m unittest discover -s scripts -p "test_*.py"
"""
import shutil
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


@unittest.skipIf(fontTools is None, "fontTools is not installed (CI skips this test)")
class GenSetiIcons(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
