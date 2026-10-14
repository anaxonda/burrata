import io
import email.message
import pathlib
import importlib.util
import tempfile
import unittest
import urllib.error
import zipfile

SCRIPT = pathlib.Path(__file__).with_name("sign-firefox.py")
SPEC = importlib.util.spec_from_file_location("sign_firefox", SCRIPT)
sign_firefox = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sign_firefox)
SignFirefoxError = sign_firefox.SignFirefoxError
api_get_version = sign_firefox.api_get_version
assert_signed_payload_matches = sign_firefox.assert_signed_payload_matches
TrustedDownloadRedirect = sign_firefox.TrustedDownloadRedirect


class Response:
    status = 200

    def __init__(self, payload):
        self.payload = payload

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def read(self, _limit):
        return self.payload


class SignFirefoxTests(unittest.TestCase):
    def make_archive(self, path: pathlib.Path, *, changed=False, signatures=False,
                     extra_meta_inf=False):
        with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("manifest.json", b'{"version":"0.1.8"}')
            archive.writestr("background.js", b"changed" if changed else b"same")
            if signatures:
                archive.writestr("META-INF/manifest.mf", b"manifest signature data")
                archive.writestr("META-INF/mozilla.sf", b"signature data")
                archive.writestr("META-INF/mozilla.rsa", b"signature certificate")
                archive.writestr("META-INF/cose.manifest", b"COSE manifest")
                archive.writestr("META-INF/cose.sig", b"COSE signature")
            if extra_meta_inf:
                archive.writestr("META-INF/injected.js", b"unexpected payload")

    def test_matching_payload_ignores_only_mozilla_signature_files(self):
        with tempfile.TemporaryDirectory() as name:
            source = pathlib.Path(name) / "source.zip"
            signed = pathlib.Path(name) / "signed.xpi"
            self.make_archive(source)
            self.make_archive(signed, signatures=True)
            assert_signed_payload_matches(source, signed)

    def test_different_payload_requires_extension_version_bump(self):
        with tempfile.TemporaryDirectory() as name:
            source = pathlib.Path(name) / "source.zip"
            signed = pathlib.Path(name) / "signed.xpi"
            self.make_archive(source)
            self.make_archive(signed, changed=True, signatures=True)
            with self.assertRaisesRegex(SignFirefoxError, "Bump extension/manifest.json version"):
                assert_signed_payload_matches(source, signed)

    def test_existing_xpi_without_all_amo_signatures_is_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            source = pathlib.Path(name) / "source.zip"
            signed = pathlib.Path(name) / "signed.xpi"
            self.make_archive(source)
            self.make_archive(signed)
            with self.assertRaisesRegex(SignFirefoxError, "missing required Mozilla signature"):
                assert_signed_payload_matches(source, signed)

    def test_arbitrary_meta_inf_file_is_not_treated_as_a_signature(self):
        with tempfile.TemporaryDirectory() as name:
            source = pathlib.Path(name) / "source.zip"
            signed = pathlib.Path(name) / "signed.xpi"
            self.make_archive(source)
            self.make_archive(signed, signatures=True, extra_meta_inf=True)
            with self.assertRaisesRegex(SignFirefoxError, "different payload"):
                assert_signed_payload_matches(source, signed)

    def test_web_ext_diagnostic_redacts_credentials_and_keeps_safe_context(self):
        api_key = "example-amo-api-key"
        api_secret = "example-amo-api-secret"
        jwt = "aaa.bbb.ccc"
        output = (
            f"Submission failed: {api_key}\n"
            f"Authorization: JWT {jwt}\n"
            f"WEB_EXT_API_SECRET={api_secret}\n"
            'Response body: {"detail":"private body"}\n'
            "Error: The listed files are invalid.\n"
        )
        safe = sign_firefox.safe_process_diagnostic(output, None, [api_key, api_secret, jwt])
        self.assertIn("Submission failed: [REDACTED]", safe)
        self.assertIn("Error: The listed files are invalid.", safe)
        for sensitive in (api_key, api_secret, jwt, "private body"):
            self.assertNotIn(sensitive, safe)

    def test_web_ext_sign_failure_keeps_primary_logger_reason_before_stack(self):
        output = (
            "[error] WebExtError: AMO rejected submission: validation failed\n"
            '    at file:///runner/node_modules/web-ext/lib/cmd/sign.js:101:13\n'
            "    at async Program.execute (web-ext/lib/program.js:270:7)\n"
            '{"detail":"private response body"}\n'
        )
        safe = sign_firefox.safe_process_diagnostic(output, None, [])
        self.assertIn("[error] WebExtError: AMO rejected submission: validation failed", safe)
        self.assertNotIn("sign.js:101", safe)
        self.assertNotIn("private response body", safe)

    def test_404_is_a_version_miss(self):
        def open_api(_request, timeout):
            self.assertEqual(timeout, 30)
            raise urllib.error.HTTPError("https://addons.mozilla.org/", 404, "missing", {}, io.BytesIO(b""))

        self.assertIsNone(api_get_version("addon@example.invalid", "0.1.8", "fake-jwt", open_api=open_api))

    def test_401_is_an_auth_error_not_a_version_miss_and_body_is_not_logged(self):
        secret_body = b"credential body must not appear in logs"

        def open_api(_request, timeout):
            raise urllib.error.HTTPError("https://addons.mozilla.org/", 401, "unauthorized", {}, io.BytesIO(secret_body))

        with self.assertRaises(SignFirefoxError) as caught:
            api_get_version("addon@example.invalid", "0.1.8", "fake-jwt", open_api=open_api)
        self.assertIn("HTTP 401", str(caught.exception))
        self.assertNotIn(secret_body.decode(), str(caught.exception))

    def test_authorization_is_removed_when_redirecting_to_amo_cdn(self):
        request = sign_firefox.urllib.request.Request(
            "https://addons.mozilla.org/firefox/downloads/file/example.xpi",
            headers={"Authorization": "JWT test-token"},
        )
        redirected = TrustedDownloadRedirect().redirect_request(
            request, None, 302, "Found", email.message.Message(),
            "https://addons.cdn.mozilla.net/user-media/example.xpi",
        )
        self.assertIsNotNone(redirected)
        self.assertNotIn("Authorization", redirected.headers)


if __name__ == "__main__":
    unittest.main()
