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

    def test_provider_accessors_are_not_mistaken_for_literal_credentials(self):
        self.assertEqual([], self.scan("const apiKey = url.searchParams.get('key');"))
        self.assertEqual([], self.scan("return { accessToken: response.accessToken };"))

    def test_quoted_and_environment_provider_credentials_are_detected(self):
        token = "Long" + "Credential" + "Value12345"
        self.assertIn((1, "provider-api-key"), self.scan(f'const accessToken = "{token}";'))
        self.assertIn((1, "provider-api-key-bare"), self.scan(f"ACCESS_TOKEN={token}"))

    def test_quoted_json_credential_key_is_detected(self):
        token = "Json" + "Credential" + "Value12345"
        self.assertIn((1, "provider-api-key"), self.scan(f'{{"accessToken":"{token}"}}'))

    def test_explicit_fixture_placeholder_is_allowed_but_other_test_values_are_scanned(self):
        self.assertEqual([], self.scan('const accessToken = "TEST-FIXTURE-CREDENTIAL-0123456789";'))
        token = "Unmarked" + "Synthetic" + "Credential123"
        self.assertIn((1, "provider-api-key"), self.scan(f'const accessToken = "{token}";'))

    def test_fixture_marker_elsewhere_on_line_does_not_hide_a_credential(self):
        token = "Unmarked" + "Synthetic" + "Credential123"
        content = f'const fixture = "TEST-FIXTURE-CREDENTIAL-0123456789"; const accessToken = "{token}";'
        self.assertIn((1, "provider-api-key"), self.scan(content))

    def test_fixture_value_does_not_hide_second_credential_on_same_line(self):
        token = "Unmarked" + "Synthetic" + "Credential123"
        content = f'{{"apiKey":"TEST-FIXTURE-CREDENTIAL-0123456789","password":"{token}"}}'
        self.assertIn((1, "provider-api-key"), self.scan(content))


if __name__ == "__main__":
    unittest.main()
