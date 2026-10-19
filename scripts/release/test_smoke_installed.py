#!/usr/bin/env python3
"""Focused tests for installed smoke runner path handling."""
import importlib.util
import pathlib
import plistlib
import sqlite3
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


class MacosTransportSecurityTests(unittest.TestCase):
    def _write_bundle(self, info: dict) -> pathlib.Path:
        temp = tempfile.TemporaryDirectory(prefix="burrata-ats-test-")
        self.addCleanup(temp.cleanup)
        app = pathlib.Path(temp.name) / "Burrata.app"
        contents = app / "Contents"
        contents.mkdir(parents=True)
        with (contents / "Info.plist").open("wb") as stream:
            plistlib.dump(info, stream)
        return app

    def test_installed_plist_accepts_only_loopback_http_exception(self) -> None:
        app = self._write_bundle({
            "NSAppTransportSecurity": {
                "NSExceptionDomains": {
                    "127.0.0.1": {"NSExceptionAllowsInsecureHTTPLoads": True},
                },
            },
        })
        self.assertEqual(
            smoke.macos_transport_security_check(app),
            {"loopbackHttpException": "127.0.0.1", "broadExemptions": "absent"},
        )

    def test_installed_plist_rejects_broad_or_other_domain_exemptions(self) -> None:
        cases = [
            {"NSAllowsArbitraryLoads": True},
            {"NSAllowsLocalNetworking": True},
            {"NSExceptionDomains": {
                "127.0.0.1": {"NSExceptionAllowsInsecureHTTPLoads": True},
                "localhost": {"NSExceptionAllowsInsecureHTTPLoads": True},
            }},
        ]
        for ats in cases:
            with self.subTest(ats=ats):
                app = self._write_bundle({"NSAppTransportSecurity": ats})
                with self.assertRaises(smoke.SmokeFailure):
                    smoke.macos_transport_security_check(app)


class InstalledWebviewAudioTests(unittest.TestCase):
    @staticmethod
    def _sample(track_id: str, current_time: float, *, error_code=None,
                entry_id="smoke-entry", source_stable=True, paused=False) -> dict:
        return {
            "engine": "webview", "applicable": True, "trackId": track_id,
            "queueTrackId": track_id, "queueTotal": 1, "entryId": entry_id,
            "sourceGeneration": 3, "previewKind": "local", "playbackStatus": "playing",
            "media": {"present": True, "currentTime": current_time, "duration": 4.0,
                      "paused": paused, "readyState": 4, "networkState": 1,
                      "errorCode": error_code, "sourcePresent": True,
                      "sourceStable": source_stable},
        }

    def test_fixture_registration_is_local_and_stores_track_asset_membership(self) -> None:
        with tempfile.TemporaryDirectory(prefix="burrata-webview-audio-") as directory:
            root = pathlib.Path(directory)
            app_data = root / "data" / "org.burrata.discovery"
            audio = root / "fixture" / "tone.m4a"
            audio.parent.mkdir()
            audio.write_bytes(b"synthetic-aac-fixture")
            fixture = smoke.register_webview_audio_fixture(app_data, audio, 4.0, root)
            with sqlite3.connect(app_data / "burrata.sqlite") as connection:
                listing = connection.execute(
                    "SELECT provider,provider_id,recording_id,source_url,metadata FROM catalog_listings"
                ).fetchone()
                asset = connection.execute(
                    "SELECT recording_id,path,hash,payload FROM local_assets"
                ).fetchone()
            self.assertEqual(listing[0], "local")
            self.assertEqual(listing[1], listing[2].removeprefix("local:"))
            self.assertEqual(listing[2], fixture["trackId"])
            self.assertTrue(pathlib.Path(asset[1]).is_relative_to(root))
            self.assertEqual(asset[0], listing[2])
            self.assertEqual(asset[2], listing[1])
            self.assertEqual(__import__("json").loads(asset[3])["track"]["assetId"],
                             __import__("json").loads(listing[4])["assetId"])

    def test_clock_progress_requires_same_playing_track_and_entry(self) -> None:
        first = self._sample("local:fixture", 0.2)
        second = self._sample("local:fixture", 0.9)
        result = smoke.assess_webview_audio_progress("local:fixture", first, second)
        self.assertGreaterEqual(result["clockAdvanceSeconds"], 0.5)

        with self.assertRaises(smoke.SmokeFailure):
            smoke.assess_webview_audio_progress("local:fixture", first,
                                                self._sample("local:fixture", 0.3))
        changed_entry = self._sample("local:fixture", 1.0, entry_id="other-entry")
        with self.assertRaises(smoke.SmokeFailure):
            smoke.assess_webview_audio_progress("local:fixture", first, changed_entry)
        wrong_track = self._sample("local:other", 1.0)
        with self.assertRaises(smoke.SmokeFailure):
            smoke.assess_webview_audio_progress("local:fixture", first, wrong_track)

    def test_source_failure_and_unsupported_media_are_distinct(self) -> None:
        self.assertEqual(smoke.webview_audio_failure_category(2), "source_failure")
        self.assertEqual(smoke.webview_audio_failure_category(4), "unsupported_media")
        first = self._sample("local:fixture", 0.2, error_code=2)
        with self.assertRaisesRegex(smoke.SmokeFailure, "source_failure"):
            smoke.assess_webview_audio_progress("local:fixture", first,
                                                self._sample("local:fixture", 0.9))
        unsupported = self._sample("local:fixture", 0.2, error_code=4)
        with self.assertRaisesRegex(smoke.SmokeFailure, "unsupported_media"):
            smoke.assess_webview_audio_progress("local:fixture", unsupported,
                                                self._sample("local:fixture", 0.9))


if __name__ == "__main__":
    unittest.main()
