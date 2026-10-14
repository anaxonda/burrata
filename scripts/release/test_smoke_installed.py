#!/usr/bin/env python3
"""Focused tests for installed smoke runner path handling."""
import importlib.util
import pathlib
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("smoke-installed.py")
SPEC = importlib.util.spec_from_file_location("smoke_installed", SCRIPT)
smoke = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(smoke)


class CanonicalTempRootTests(unittest.TestCase):
    def test_symlinked_temp_root_resolves_before_app_launch(self) -> None:
        with tempfile.TemporaryDirectory(prefix="burrata-path-test-") as directory:
            root = pathlib.Path(directory)
            real = root / "real"
            alias = root / "alias"
            real.mkdir()
            try:
                alias.symlink_to(real, target_is_directory=True)
            except (NotImplementedError, OSError) as error:
                self.skipTest(f"directory symlinks unavailable: {error}")

            canonical = smoke.canonical_temp_root(alias)
            self.assertEqual(canonical, real.resolve(strict=True))
            self.assertFalse(canonical.is_symlink())


if __name__ == "__main__":
    unittest.main()
