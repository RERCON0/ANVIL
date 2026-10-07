"""Tests for the package checks that need no built package.

Run: python -B -m unittest discover -s scripts -p "test_*.py"
"""
from pathlib import Path
import subprocess
import tempfile
import unittest

import check_package


class BuildPathGuard(unittest.TestCase):
    def test_rejects_executables_that_embed_the_builders_directories(self):
        home = str(Path.home())
        for leaked in (home, home.upper(), home.replace("\\", "/"), str(check_package.ROOT)):
            with self.subTest(leaked=leaked):
                data = b"MZ\0\0panic at " + leaked.encode() + b"\\.cargo\\registry\\src\\x\\lib.rs\0"
                with self.assertRaises(ValueError):
                    check_package.check_no_build_paths(data, "anvil.exe")

    def test_accepts_remapped_paths(self):
        check_package.check_no_build_paths(b"MZ\0\0at /cargo/registry/src/x/lib.rs, /anvil/src/app.rs, /rust/library\0", "anvil.exe")


class WorkingTreeCleanliness(unittest.TestCase):
    def git(self, root, *args):
        command = ["git", "-c", "user.name=t", "-c", "user.email=t@example.com", *args]
        subprocess.run(command, cwd=root, check=True, capture_output=True)

    def test_untracked_files_count_but_ignored_build_output_does_not(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.git(root, "init", "-q")
            (root / ".gitignore").write_text("\n".join(["/target", "/dist", "__pycache__/", ""]))
            (root / "build.rs").write_text("fn main() {}\n")
            self.git(root, "add", ".gitignore", "build.rs")
            self.git(root, "commit", "-q", "-m", "base")
            self.assertTrue(check_package.tree_is_clean(root))

            for ignored in ("target/release/x.rlib", "dist/stage-1/anvil.exe", "scripts/__pycache__/x.pyc"):
                (root / ignored).parent.mkdir(parents=True, exist_ok=True)
                (root / ignored).write_text("x")
            self.assertTrue(check_package.tree_is_clean(root), "ignored build output is not a source change")

            (root / "scripts" / "collect_licenses.py").write_text("print()\n")
            self.assertFalse(check_package.tree_is_clean(root), "a new source file Git has not seen")
            (root / "scripts" / "collect_licenses.py").unlink()
            self.assertTrue(check_package.tree_is_clean(root))

            (root / "build.rs").write_text("fn main() { panic!() }\n")
            self.assertFalse(check_package.tree_is_clean(root), "a tracked modification")


if __name__ == "__main__":
    unittest.main()
