#!/usr/bin/env python3
"""Regression checks using synthetic credentials assembled at runtime."""
import contextlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import audit_secrets as audit


class SecretAuditTests(unittest.TestCase):
    def scan(self, content):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source.rs"
            path.write_text(content, encoding="utf-8")
            return audit.scan_file(path)

    def test_google_key_in_inline_query_is_detected(self):
        token = "AI" + "za" + "A" * 35
        self.assertIn((1, "google-api-key"), self.scan(f'.query(&[("key", "{token}")])'))

    def test_google_key_can_end_in_nonword_character(self):
        token = "AI" + "za" + "A" * 34 + "-"
        self.assertIn((1, "google-api-key"), self.scan(f'"{token}"'))

    def test_partial_or_longer_tokens_do_not_match(self):
        for length in (34, 36):
            token = "AI" + "za" + "A" * length
            self.assertNotIn((1, "google-api-key"), self.scan(f'"{token}"'))

    def test_report_identifies_location_without_echoing_secret(self):
        token = "AI" + "za" + "A" * 35
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "source.rs"
            path.write_text(f'"{token}"', encoding="utf-8")
            output = io.StringIO()
            with patch.object(audit, "ROOT", root), patch.object(audit, "tracked_paths", return_value=[path]), contextlib.redirect_stdout(output):
                self.assertEqual(audit.main(), 1)
            self.assertIn("source.rs:1: google-api-key", output.getvalue())
            self.assertNotIn(token, output.getvalue())


if __name__ == "__main__":
    unittest.main()
