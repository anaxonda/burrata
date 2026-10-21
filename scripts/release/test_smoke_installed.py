#!/usr/bin/env python3
"""Focused tests for installed smoke runner path handling."""
import importlib.util
from contextlib import closing, ExitStack
from unittest.mock import patch
import pathlib
import json
import subprocess
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

    def _run_installed_bundle_smoke(self, app_bundle: pathlib.Path, *, audio_failure=None):
        executable = app_bundle / "Contents/MacOS/burrata"
        executable.parent.mkdir(parents=True)
        executable.write_bytes(b"synthetic application executable")
        resources = app_bundle / "Contents/Resources"
        resources.mkdir()
        installer = app_bundle.parent / "Burrata.dmg"
        installer.write_bytes(b"synthetic installer")
        evidence_path = app_bundle.parent / "smoke-evidence.json"
        with ExitStack() as patches:
            patches.enter_context(patch.object(smoke.sys, "platform", "darwin"))
            patches.enter_context(patch.object(smoke.sys, "argv", [
                str(SCRIPT), "--installer", str(installer), "--evidence", str(evidence_path),
            ]))
            patches.enter_context(patch.object(smoke, "install_macos",
                                               return_value=(executable, resources, str(app_bundle))))
            for name in ["run", "stop_process", "bundled_tool_checks", "bundled_media_checks",
                         "wait_for_app", "native_host_check", "run_mcp_status", "run_mcp_webview_audio"]:
                mocked = patches.enter_context(patch.object(smoke, name, return_value={}))
                if name == "run_mcp_webview_audio" and audio_failure is not None:
                    mocked.side_effect = audio_failure
            patches.enter_context(patch.object(smoke, "build_webview_audio_fixture",
                                               return_value={"trackId": "local:fixture"}))
            launch = patches.enter_context(patch.object(smoke, "launch_app"))
            status = smoke.main()
            launches = launch.call_args_list
        evidence = __import__("json").loads(evidence_path.read_text())
        return status, evidence, launches, executable

    def test_main_checks_installed_bundle_while_launching_its_executable(self) -> None:
        app = self._write_bundle({
            "NSAppTransportSecurity": {
                "NSExceptionDomains": {
                    "127.0.0.1": {"NSExceptionAllowsInsecureHTTPLoads": True},
                },
            },
        })
        status, evidence, launches, executable = self._run_installed_bundle_smoke(app)
        self.assertEqual(status, 0)
        self.assertEqual(evidence["status"], "passed")
        self.assertEqual(evidence["checks"]["transportSecurity"],
                         {"loopbackHttpException": "127.0.0.1", "broadExemptions": "absent"})
        self.assertEqual(len(launches), 1)
        self.assertEqual(launches[0].args[0], executable)

    def test_main_retains_audio_failure_diagnostics_without_marking_smoke_passed(self) -> None:
        app = self._write_bundle({"NSAppTransportSecurity": {"NSExceptionDomains": {
            "127.0.0.1": {"NSExceptionAllowsInsecureHTTPLoads": True},
        }}})
        failure = smoke.SmokeFailure("WebView audio", "WebView audio clock did not advance by 0.5 seconds")
        failure.diagnostics = {"first": {"media": {"currentTime": 0.2}}, "second": {"media": {"currentTime": 0.2}}}
        status, evidence, _, _ = self._run_installed_bundle_smoke(app, audio_failure=failure)
        self.assertEqual(status, 1)
        self.assertEqual(evidence["status"], "failed")
        self.assertEqual(evidence["checks"]["webviewAudio"]["status"], "failed")
        self.assertEqual(evidence["checks"]["webviewAudio"]["diagnostics"], failure.diagnostics)
        self.assertEqual(evidence["checks"]["webviewAudio"]["mediaLogTail"], {"stderr": [], "stdout": []})

    def test_main_rejects_broad_installed_bundle_ats_before_launch(self) -> None:
        app = self._write_bundle({
            "NSAppTransportSecurity": {
                "NSExceptionDomains": {
                    "127.0.0.1": {"NSExceptionAllowsInsecureHTTPLoads": True},
                },
                "NSAllowsArbitraryLoads": True,
            },
        })
        status, evidence, launches, _ = self._run_installed_bundle_smoke(app)
        self.assertEqual(status, 1)
        self.assertEqual(evidence["failedStage"], "macOS ATS")
        self.assertEqual(evidence["error"], "broad ATS transport exemption is present")
        self.assertEqual(launches, [])


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
            closed = []
            class TrackedConnection(sqlite3.Connection):
                def close(self):
                    closed.append(True)
                    super().close()
            connect = sqlite3.connect
            with patch.object(smoke.sqlite3, "connect",
                              side_effect=lambda *a, **kw: connect(*a, factory=TrackedConnection, **kw)):
                fixture = smoke.register_webview_audio_fixture(app_data, audio, 4.0, root)
            self.assertEqual(closed, [True], "fixture registration must release its SQLite handle")
            with closing(sqlite3.connect(app_data / "burrata.sqlite")) as connection:
                listing = connection.execute(
                    "SELECT provider,provider_id,recording_id,source_url,metadata FROM catalog_listings"
                ).fetchone()
                asset = connection.execute(
                    "SELECT recording_id,path,hash,payload FROM local_assets"
                ).fetchone()
            self.assertEqual(listing[0], "local")
            self.assertEqual(listing[1], listing[2].removeprefix("local:"))
            self.assertEqual(listing[2], fixture["trackId"])
            self.assertTrue(pathlib.Path(asset[1]).resolve(strict=True).is_relative_to(root.resolve(strict=True)))
            self.assertEqual(asset[0], listing[2])
            self.assertEqual(asset[2], listing[1])
            self.assertEqual(__import__("json").loads(asset[3])["track"]["assetId"],
                             __import__("json").loads(listing[4])["assetId"])

    def test_fixture_registration_accepts_canonicalized_temporary_alias(self) -> None:
        with tempfile.TemporaryDirectory(prefix="burrata-fixture-alias-") as directory:
            real = pathlib.Path(directory) / "real"
            real.mkdir()
            alias = pathlib.Path(directory) / "alias"
            try:
                alias.symlink_to(real, target_is_directory=True)
            except (NotImplementedError, OSError) as error:
                self.skipTest(f"directory symlinks unavailable: {error}")
            audio = alias / "tone.m4a"
            audio.write_bytes(b"synthetic-aac-fixture")
            smoke.register_webview_audio_fixture(alias / "data", audio, 4.0, alias)
            with closing(sqlite3.connect(real / "data" / "burrata.sqlite")) as connection:
                saved = pathlib.Path(connection.execute("SELECT path FROM local_assets").fetchone()[0])
            self.assertEqual(saved, audio.resolve(strict=True))
            self.assertTrue(saved.is_relative_to(alias.resolve(strict=True)))

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


class InstalledWebviewDiagnosticRetentionTests(unittest.TestCase):
    def test_audio_checker_outer_timeout_covers_all_documented_gate_budgets(self):
        with tempfile.TemporaryDirectory(prefix="burrata-audio-budget-") as directory:
            root = pathlib.Path(directory)
            target = "synthetic-target"
            tools = root / "tools" / target
            tools.mkdir(parents=True)
            (tools / ("burrata-mcp.exe" if smoke.sys.platform == "win32" else "burrata-mcp")).touch()
            completed = subprocess.CompletedProcess([], 1, json.dumps({
                "status": "failed", "failureCategory": "audio_not_ready",
            }).encode(), b"")
            with patch.object(smoke.shutil, "which", return_value="synthetic-node"), \
                    patch.object(smoke, "run", return_value=completed) as runner:
                with self.assertRaises(smoke.SmokeFailure):
                    smoke.run_mcp_webview_audio(root, target, root / "automation.json", {}, "local:fixture")

        self.assertEqual(runner.call_args.kwargs["timeout"], smoke.WEBVIEW_AUDIO_CHECK_TIMEOUT_SECONDS)
        # Frontend readiness + mute/play/pause Tauri ceilings + playback gate
        # + inspection and steady-sample time + bounded IPC/process slack.
        required = 30 + (3 * 45) + 10 + 5 + 1 + 20
        self.assertGreaterEqual(smoke.WEBVIEW_AUDIO_CHECK_TIMEOUT_SECONDS, required)

    def _run_payload(self, payload, returncode=0):
        with tempfile.TemporaryDirectory(prefix="burrata-audio-evidence-") as directory:
            root = pathlib.Path(directory)
            target = "synthetic-target"
            tools = root / "tools" / target
            tools.mkdir(parents=True)
            (tools / ("burrata-mcp.exe" if smoke.sys.platform == "win32" else "burrata-mcp")).touch()
            completed = subprocess.CompletedProcess([], returncode, json.dumps(payload).encode(), b"")
            with patch.object(smoke.shutil, "which", return_value="synthetic-node"), patch.object(smoke, "run", return_value=completed):
                return smoke.run_mcp_webview_audio(root, target, root / "automation.json", {}, "local:fixture")

    def test_clock_failure_keeps_both_media_samples_and_actual_wall_timing(self) -> None:
        first = InstalledWebviewAudioTests._sample("local:fixture", 0.2)
        second = InstalledWebviewAudioTests._sample("local:fixture", 0.2)
        for sample in (first, second):
            sample["media"].update({"ended": False, "seeking": False, "buffered": [[0, 4]], "seekable": [[0, 4]]})
        payload = {"status": "sampled", "first": first, "second": second, "latest": second,
                   "sampling": {"firstSampleElapsedMs": 400, "secondSampleElapsedMs": 1325, "betweenSamplesWallMs": 925, "totalElapsedMs": 1330}}
        with self.assertRaisesRegex(smoke.SmokeFailure, "did not advance by 0.5 seconds") as caught:
            self._run_payload(payload)
        diagnostics = caught.exception.diagnostics
        self.assertEqual(diagnostics["first"]["media"]["currentTime"], 0.2)
        self.assertEqual(diagnostics["second"]["media"]["currentTime"], 0.2)
        self.assertEqual(diagnostics["sampling"]["betweenSamplesWallMs"], 925)
        self.assertIs(diagnostics["second"]["media"]["ended"], False)
        self.assertIs(diagnostics["second"]["media"]["seeking"], False)
        self.assertEqual(diagnostics["second"]["media"]["buffered"], [[0, 4]])

    def test_initial_readiness_and_first_clock_progress_survive_startup_failure(self) -> None:
        initial = InstalledWebviewAudioTests._sample("local:fixture", 0.013834)
        progress = InstalledWebviewAudioTests._sample("local:fixture", 0.61)
        with self.assertRaisesRegex(smoke.SmokeFailure, "steady playback") as caught:
            self._run_payload({"status": "failed", "failureCategory": "steady_clock_stalled",
                               "initialReady": initial, "firstProgress": progress,
                               "first": progress, "second": InstalledWebviewAudioTests._sample("local:fixture", 0.71),
                               "sampling": {"initialReadyElapsedMs": 744.6, "firstProgressElapsedMs": 5400}}, 1)
        self.assertEqual(caught.exception.diagnostics["initialReady"]["media"]["currentTime"], 0.013834)
        self.assertEqual(caught.exception.diagnostics["firstProgress"]["media"]["currentTime"], 0.61)
        self.assertEqual(caught.exception.diagnostics["sampling"]["initialReadyElapsedMs"], 744.6)
        self.assertEqual(caught.exception.diagnostics["sampling"]["firstProgressElapsedMs"], 5400)

    def test_passing_evidence_retains_startup_proof_and_steady_samples(self) -> None:
        initial = InstalledWebviewAudioTests._sample("local:fixture", 0.013834)
        first = InstalledWebviewAudioTests._sample("local:fixture", 0.61)
        second = InstalledWebviewAudioTests._sample("local:fixture", 1.51)
        evidence = self._run_payload({"status": "sampled", "initialReady": initial, "firstProgress": first,
                                      "first": first, "second": second, "latest": second,
                                      "sampling": {"initialReadyElapsedMs": 744.6, "firstProgressElapsedMs": 2200, "betweenSamplesWallMs": 900}})
        self.assertEqual(evidence["clockAdvanceSeconds"], 0.9)
        self.assertEqual(evidence["diagnostics"]["initialReady"]["media"]["currentTime"], 0.013834)
        self.assertEqual(evidence["diagnostics"]["firstProgress"]["media"]["currentTime"], 0.61)
        self.assertEqual(evidence["diagnostics"]["sampling"]["betweenSamplesWallMs"], 900)

    def test_early_media_error_retains_partial_latest_sample(self) -> None:
        latest = InstalledWebviewAudioTests._sample("local:fixture", 0, error_code=4)
        with self.assertRaisesRegex(smoke.SmokeFailure, "unsupported_media") as caught:
            self._run_payload({"status": "failed", "failureCategory": "unsupported_media", "first": None, "second": None, "latest": latest}, 1)
        self.assertIsNone(caught.exception.diagnostics["first"])
        self.assertEqual(caught.exception.diagnostics["latest"]["media"]["errorCode"], 4)
        self.assertIsNone(caught.exception.diagnostics["latest"]["media"]["ended"])

    def test_retention_allows_only_bounded_media_fields_and_no_source_material(self) -> None:
        sample = InstalledWebviewAudioTests._sample("https://private.invalid/?token=track-secret", 0.2, entry_id="token=entry-secret")
        sample.update({"url": "https://private.invalid/?token=url-secret", "message": "Authorization: Bearer bearer-secret"})
        sample["media"].update({"currentSrc": "file:///private/file-secret", "errorMessage": "password=error-secret", "buffered": [[0, 4]] * 100, "seekable": [[0, float("inf")], [False, 4], [0, 4]], "currentTime": float("nan")})
        diagnostics = smoke.safe_webview_audio_diagnostics({"first": sample, "sampling": {"totalElapsedMs": float("inf"), "url": "https://private.invalid"}}, "local:fixture")
        encoded = json.dumps(diagnostics)
        for secret in ("private.invalid", "track-secret", "entry-secret", "url-secret", "bearer-secret", "file-secret", "error-secret"):
            self.assertNotIn(secret, encoded)
        self.assertLess(len(encoded), 4000)
        self.assertEqual(len(diagnostics["first"]["media"]["buffered"]), 8)
        self.assertEqual(diagnostics["first"]["media"]["seekable"], [[0, 4]])
        self.assertIsNone(diagnostics["first"]["media"]["currentTime"])
        self.assertIsNone(diagnostics["sampling"]["totalElapsedMs"])

    def test_platform_log_tail_is_bounded_and_sensitive_lines_are_fully_redacted(self) -> None:
        with tempfile.TemporaryDirectory(prefix="burrata-audio-log-") as directory:
            path = pathlib.Path(directory) / "app.log"
            path.write_text("ignored untrusted metadata\n" + "GStreamer pipeline waiting for clock\n" * 100 + "WebKit failed https://private.invalid/?token=url-secret\nALSA token=token-secret\nPulseAudio file:///tmp/path-secret\nGStreamer " + "x" * 1000)
            lines = smoke.safe_media_log_tail(path)
            self.assertLessEqual(len(lines), 20)
            self.assertTrue(all(len(line) <= 200 for line in lines))
            encoded = json.dumps(lines)
            for secret in ("private.invalid", "url-secret", "token-secret", "path-secret", "untrusted metadata"):
                self.assertNotIn(secret, encoded)
            self.assertIn("[redacted media diagnostic line]", lines)
            self.assertIn("GStreamer pipeline waiting for clock", lines)


class LinuxAudioDiagnosticTests(unittest.TestCase):
    def test_sink_diagnostics_keep_only_availability_and_bounded_state_counts(self) -> None:
        info = subprocess.CompletedProcess([], 0, b"Server String: /private/server\nCookie: token-secret\nDefault Sink: private-sink-name\n", b"")
        sinks = subprocess.CompletedProcess([], 0, b"1\tprivate-sink-name\tdriver\ts16le 2ch 44100Hz\tIDLE\n2\tother-private-name\tdriver\ts16le 2ch 44100Hz\tRUNNING\n", b"")
        with patch.object(smoke.shutil, "which", return_value="synthetic-pactl"), patch.object(smoke, "run", side_effect=[info, sinks]) as runner:
            diagnostics = smoke.linux_audio_diagnostics({"PATH": "/usr/bin"})
        self.assertIs(diagnostics["serverReachable"], True)
        self.assertIs(diagnostics["defaultSinkPresent"], True)
        self.assertEqual(diagnostics["sinkCount"], 2)
        self.assertEqual(diagnostics["sinkStates"], {"RUNNING": 1, "IDLE": 1, "SUSPENDED": 0, "UNKNOWN": 0})
        for value in ("private", "token-secret", "server", "sink-name"):
            self.assertNotIn(value, json.dumps(diagnostics).replace("serverReachable", ""))
        self.assertEqual(runner.call_args_list[0].kwargs["env"]["LC_ALL"], "C")
        self.assertEqual(runner.call_args_list[0].kwargs["timeout"], 5)

    def test_missing_or_unreachable_pulse_server_is_diagnostic_only(self) -> None:
        with patch.object(smoke.shutil, "which", return_value=None):
            self.assertEqual(smoke.linux_audio_diagnostics({}), {"available": False})
        failed = subprocess.CompletedProcess([], 1, b"", b"Authorization cookie=secret")
        with patch.object(smoke.shutil, "which", return_value="synthetic-pactl"), patch.object(smoke, "run", return_value=failed):
            diagnostics = smoke.linux_audio_diagnostics({})
        self.assertIs(diagnostics["serverReachable"], False)
        self.assertIs(diagnostics["sinkListingSucceeded"], False)
        self.assertEqual(diagnostics["sinkCount"], 0)
        self.assertNotIn("secret", json.dumps(diagnostics))


if __name__ == "__main__":
    unittest.main()
