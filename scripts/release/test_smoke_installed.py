#!/usr/bin/env python3
"""Focused tests for installed smoke runner path handling."""
import importlib.util
from contextlib import closing, ExitStack
from unittest.mock import patch, Mock
import pathlib
import json
import subprocess
import plistlib
import sqlite3
import tempfile
import unittest
import math
import struct


SCRIPT = pathlib.Path(__file__).with_name("smoke-installed.py")
SPEC = importlib.util.spec_from_file_location("smoke_installed", SCRIPT)
smoke = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(smoke)
RUNTIME_SPEC = importlib.util.spec_from_file_location(
    "appimage_runtime", SCRIPT.with_name("prepare-appimage-config.py"))
runtime = importlib.util.module_from_spec(RUNTIME_SPEC)
assert RUNTIME_SPEC.loader is not None
RUNTIME_SPEC.loader.exec_module(runtime)


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


class AppImageTests(unittest.TestCase):
    def test_extracts_and_launches_wrapper_instead_of_bypassing_its_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            installer = root / "Burrata.AppImage"
            installer.write_bytes(b"fixture")

            def extract(command, **kwargs):
                if command[0] == "ldd":
                    self.assertEqual(command[1], "-r")
                    return subprocess.CompletedProcess(command, 0, stdout=b"", stderr=b"")
                self.assertEqual(command, [str(installer), "--appimage-extract"])
                appdir = kwargs["cwd"] / "squashfs-root"
                mcp = appdir / "usr/lib/Burrata/resources/mcp/server.cjs"
                mcp.parent.mkdir(parents=True)
                mcp.write_text("fixture")
                (appdir / "AppRun").write_text("fixture")
                return subprocess.CompletedProcess(command, 0)

            with patch.object(smoke.subprocess, "run", side_effect=extract):
                app, resources, package = smoke.install_linux(installer, root)
            self.assertEqual(app.name, "AppRun")
            self.assertEqual(resources, app.parent / "usr/lib/Burrata/resources")
            self.assertIsNone(package)

    def test_incomplete_appimage_cannot_pass_installation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            with patch.object(smoke.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)):
                with self.assertRaises(smoke.SmokeFailure):
                    smoke.install_linux(root / "Burrata.AppImage", root)


class PrivateLinuxAudioTests(unittest.TestCase):
    def test_private_endpoint_replaces_inherited_server_without_global_daemon_commands(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory).resolve()
            env = {"PULSE_SERVER": "unix:/user/audio", "PATH": "/bin"}
            process = Mock()
            process.poll.return_value = None
            ready = subprocess.CompletedProcess([], 0, b"0\tburrata_smoke\tmodule-null-sink.c\ts16le\tIDLE\n", b"")
            with patch.object(smoke.shutil, "which", side_effect=lambda name: '/bin/' + name), \
                 patch.object(smoke.subprocess, "Popen", return_value=process) as launch, \
                 patch.object(smoke, "run", return_value=ready) as run:
                self.assertIs(smoke.start_linux_audio(env, root), process)
            command = launch.call_args.args[0]
            self.assertIn("--use-pid-file=no", command)
            self.assertIn("-n", command)
            self.assertTrue(any("norewinds=1" in value for value in command))
            self.assertNotIn("--start", command)
            self.assertNotIn("--kill", command)
            self.assertEqual(env["PULSE_SERVER"], 'unix:' + str(root / 'pulse-runtime/native'))
            self.assertTrue(launch.call_args.kwargs["start_new_session"])
            self.assertEqual(run.call_args.kwargs["env"]["PULSE_SERVER"], env["PULSE_SERVER"])

    def test_failed_private_server_stops_only_its_owned_process(self):
        with tempfile.TemporaryDirectory() as directory:
            process = Mock()
            process.poll.return_value = 1
            root = pathlib.Path(directory)
            with patch.object(smoke.shutil, "which", return_value='/bin/tool'), \
                 patch.object(smoke.subprocess, "Popen", return_value=process), \
                 patch.object(smoke, "stop_process") as stop:
                with self.assertRaisesRegex(smoke.SmokeFailure, "did not become ready"):
                    smoke.start_linux_audio({}, root)
                stop.assert_called_once_with(process, root)


class AppImageRuntimeTests(unittest.TestCase):
    def test_runtime_collection_does_not_require_gstreamer_development_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            libdir = root / 'lib'
            libdir.mkdir()
            (libdir / 'gstreamer-1.0').mkdir()
            pixbuf = root / 'loaders'
            pixbuf.mkdir()
            for names in runtime.ROOT_LIBRARIES.values():
                for name in names:
                    (libdir / name).write_bytes(b'fixture')
            gst = libdir / 'libgstreamer-1.0.so.0'
            gst.write_bytes(b'fixture')

            def pkg(package, variable):
                if package == 'gstreamer-1.0':
                    raise subprocess.CalledProcessError(1, ['pkg-config', package])
                return str(pixbuf if variable == 'gdk_pixbuf_moduledir' else libdir)

            with patch.object(runtime, 'pkg_variable', side_effect=pkg), patch.object(
                    runtime.subprocess, 'check_output', return_value=f'libgstreamer-1.0.so.0 => {gst} (0x1234)'):
                files = runtime.configuration()['bundle']['linux']['appimage']['files']
            self.assertEqual(files['/usr/lib/libgstreamer-1.0.so.0'], str(gst.resolve()))

    def test_dependency_closure_keeps_cairo_and_glib_but_leaves_host_drivers_alone(self):
        found = runtime.dependencies("""
          libcairo.so.2 => /build/lib/libcairo.so.2 (0x1234)
          libglib-2.0.so.0 => /build/lib/libglib-2.0.so.0 (0xabcd)
          libc.so.6 => /build/lib/libc.so.6 (0x1111)
          libGL.so.1 => /build/lib/libGL.so.1 (0x2222)
          libasound.so.2 => /build/lib/libasound.so.2 (0x3333)
        """)
        self.assertEqual(set(found), {"libcairo.so.2", "libglib-2.0.so.0"})

    def test_missing_dependency_fails_instead_of_falling_back_to_packagers_runtime(self):
        with self.assertRaises(ValueError):
            runtime.dependencies("libcairo.so.2 => not found")

    def test_conflicting_sonames_cannot_silently_mix_ui_runtimes(self):
        with self.assertRaises(ValueError):
            runtime.dependencies("libglib-2.0.so.0 => /first/libglib.so (0x1234)\n"
                                 "libglib-2.0.so.0 => /second/libglib.so (0x5678)")


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
                         "wait_for_app", "native_host_check", "run_mcp_status", "run_mcp_native_audio"]:
                mocked = patches.enter_context(patch.object(smoke, name, return_value={}))
                if name == "run_mcp_native_audio" and audio_failure is not None:
                    mocked.side_effect = audio_failure
            patches.enter_context(patch.object(smoke, "build_native_audio_fixture",
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
        failure = smoke.SmokeFailure("Native audio", "Native audio clock did not advance by 0.5 seconds")
        failure.diagnostics = {"first": {"media": {"currentTime": 0.2}}, "second": {"media": {"currentTime": 0.2}}}
        status, evidence, _, _ = self._run_installed_bundle_smoke(app, audio_failure=failure)
        self.assertEqual(status, 1)
        self.assertEqual(evidence["status"], "failed")
        self.assertEqual(evidence["checks"]["nativeAudio"]["status"], "failed")
        self.assertEqual(evidence["checks"]["nativeAudio"]["diagnostics"], failure.diagnostics)
        self.assertEqual(evidence["checks"]["nativeAudio"]["mediaLogTail"], {"stderr": [], "stdout": []})

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


class InstalledNativeAudioTests(unittest.TestCase):
    def test_builder_requires_aac_long_enough_for_all_steady_windows(self) -> None:
        for duration in (4.0, 12.0):
            with self.subTest(duration=duration), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                target = "synthetic-target"
                tools = root / "resources/tools" / target
                tools.mkdir(parents=True)
                ext = ".exe" if smoke.sys.platform == "win32" else ""
                for name in ("ffmpeg", "ffprobe"):
                    (tools / (name + ext)).write_bytes(b"fixture-tool")

                def command(args, **kwargs):
                    if pathlib.Path(args[0]).name == "ffmpeg" + ext:
                        pathlib.Path(args[-1]).write_bytes(b"synthetic-aac")
                        return subprocess.CompletedProcess(args, 0, b"", b"")
                    payload = {"streams": [{"codec_name": "aac"}], "format": {"duration": str(duration)}}
                    return subprocess.CompletedProcess(args, 0, json.dumps(payload).encode(), b"")

                with patch.object(smoke, "run", side_effect=command):
                    if duration == 4.0:
                        with self.assertRaisesRegex(smoke.SmokeFailure, "too short"):
                            smoke.build_native_audio_fixture(root / "resources", target, {}, root, root / "data")
                        self.assertFalse((root / "data/burrata.sqlite").exists())
                    else:
                        result = smoke.build_native_audio_fixture(root / "resources", target, {}, root, root / "data")
                        self.assertEqual(result["durationSeconds"], 12.0)


    def test_fixture_registration_is_local_and_stores_track_asset_membership(self) -> None:
        with tempfile.TemporaryDirectory(prefix="burrata-native-audio-") as directory:
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
                fixture = smoke.register_native_audio_fixture(app_data, audio, 4.0, root)
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
            smoke.register_native_audio_fixture(alias / "data", audio, 4.0, alias)
            with closing(sqlite3.connect(real / "data" / "burrata.sqlite")) as connection:
                saved = pathlib.Path(connection.execute("SELECT path FROM local_assets").fetchone()[0])
            self.assertEqual(saved, audio.resolve(strict=True))
            self.assertTrue(saved.is_relative_to(alias.resolve(strict=True)))




class InstalledMcpEntrypointTests(unittest.TestCase):
    def test_status_checker_forwards_main_mcp_arguments(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            app = root / "burrata"
            app.write_bytes(b"fixture")
            payload = {"initialized": True, "statusRunning": True}
            completed = subprocess.CompletedProcess([], 0, json.dumps(payload).encode(), b"")
            with patch.object(smoke.shutil, "which", return_value="node"), patch.object(smoke, "run", return_value=completed) as run:
                self.assertEqual(smoke.run_mcp_status(root, "target", root / "automation.json", {},
                                                     command=app, command_args=("--mcp",)), payload)
            self.assertEqual(run.call_args.args[0][-3:], [app, root / "automation.json", "--mcp"])

    def test_status_checker_retains_the_configured_resource_helper(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            helper = root / "tools/target" / ("burrata-mcp.exe" if smoke.sys.platform == "win32" else "burrata-mcp")
            helper.parent.mkdir(parents=True)
            helper.write_bytes(b"fixture")
            completed = subprocess.CompletedProcess([], 0, b'{"initialized":true}', b"")
            with patch.object(smoke.shutil, "which", return_value="node"), patch.object(smoke, "run", return_value=completed) as run:
                smoke.run_mcp_status(root, "target", root / "automation.json", {})
            self.assertEqual(run.call_args.args[0][-2:], [helper, root / "automation.json"])

    def test_checks_linux_sibling_appimage_and_windows_shipped_commands(self):
        for platform, application, console in [
            ("linux", "usr/bin/burrata", "usr/bin/burrata-mcp"),
            ("linux", "AppRun", "usr/bin/burrata-mcp"),
            ("win32", "Burrata.exe", "burrata-mcp.exe"),
        ]:
            with self.subTest(platform=platform, application=application), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                app, helper = root / application, root / console
                app.parent.mkdir(parents=True, exist_ok=True)
                helper.parent.mkdir(parents=True, exist_ok=True)
                app.write_bytes(b"fixture")
                helper.write_bytes(b"fixture")
                with patch.object(smoke.sys, "platform", platform), patch.object(smoke, "run_mcp_status", return_value={"initialized": True}) as status:
                    result = smoke.run_mcp_entrypoints(app, root / "resources", "target", root / "automation.json", {})
                self.assertEqual(status.call_count, 2)
                self.assertEqual(status.call_args_list[0].kwargs, {"command": app, "command_args": ("--mcp",)})
                self.assertEqual(status.call_args_list[1].kwargs, {"command": helper})
                self.assertEqual(len(result["shippedConsole"]), 1)

    def test_main_entrypoint_is_checked_when_no_extra_console_is_shipped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            app = root / "Burrata.app/Contents/MacOS/burrata"
            app.parent.mkdir(parents=True)
            app.write_bytes(b"fixture")
            with patch.object(smoke.sys, "platform", "darwin"), patch.object(smoke, "run_mcp_status", return_value={"initialized": True}) as status:
                result = smoke.run_mcp_entrypoints(app, root / "resources", "target", root / "automation.json", {})
            self.assertEqual(status.call_count, 1)
            self.assertEqual(status.call_args.kwargs, {"command": app, "command_args": ("--mcp",)})
            self.assertEqual(result["shippedConsole"], [])

    def test_a_failed_shipped_console_cannot_be_hidden_by_working_resource_helper(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            app, helper = root / "burrata", root / "burrata-mcp"
            app.write_bytes(b"fixture")
            helper.write_bytes(b"fixture")
            failure = smoke.SmokeFailure("mcp", "shipped command could not initialize")
            with patch.object(smoke.sys, "platform", "linux"), patch.object(smoke, "run_mcp_status", side_effect=[{"initialized": True}, failure]):
                with self.assertRaises(smoke.SmokeFailure):
                    smoke.run_mcp_entrypoints(app, root / "resources", "target", root / "automation.json", {})


class InstalledNativeOutputEvidenceTests(unittest.TestCase):
    def run_payload(self, payload):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            target = "synthetic-target"
            tools = root / "tools" / target
            tools.mkdir(parents=True)
            (tools / ("burrata-mcp.exe" if smoke.sys.platform == "win32" else "burrata-mcp")).write_bytes(b"fixture")
            result = subprocess.CompletedProcess([], 0, json.dumps(payload).encode(), b"")
            with patch.object(smoke.shutil, "which", return_value="node"), patch.object(smoke, "run", return_value=result):
                return smoke.run_mcp_native_audio(root, target, root / "automation.json", {}, "local:fixture")

    def test_clock_or_launch_alone_cannot_pass(self):
        for payload in [{"status":"sampled","engine":"native","position":100}, {"status":"failed"}, {"status":"sampled","engine":"webview"}]:
            with self.assertRaises(smoke.SmokeFailure):
                self.run_payload(payload)

    def test_requires_complete_native_stereo_measurements(self):
        payload = {"status":"sampled","engine":"native","channels":2,"capturedSeconds":3,"measurements":[{"rms":.1},{"rms":.1}]}
        self.assertEqual(self.run_payload(payload), payload)
        payload["channels"] = 1
        with self.assertRaises(smoke.SmokeFailure): self.run_payload(payload)

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


class ActualMonitorAssessmentTests(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  rate=44100;data=bytearray()
  for i in range(int(5.4*rate)):
   t=i/rate
   left,right=(.15*math.sin(math.tau*440*t),.12*math.sin(math.tau*660*t)) if .4<=t<4.2 else (0,0)
   data.extend(struct.pack('<ff',left,right))
  cls.good=bytes(data)
 def assess(self,data=None,change=None):
  data=self.good if data is None else data
  control={'verified':True,'isolatedVirtualOutput':True,'ownedAppStream':True,'physicalHardware':False,'postPlayerGain':True,'remuteWhilePlaying':True,'remuteClockAdvanceSeconds':1.0,'sampleRate':44100,'channels':2,'bytes':len(data),'unmuteOffsetSeconds':.4,'remuteOffsetSeconds':4.2}
  if change:change(control)
  with tempfile.TemporaryDirectory() as td:
   p=pathlib.Path(td)/'monitor.f32le';p.write_bytes(data);return smoke.assess_private_monitor(p,control)
 def test_actual_quiet_correct_stereo_remute_phases_pass(self):
  report=self.assess();self.assertGreaterEqual(report['verifiedToneSeconds'],3);self.assertGreaterEqual(report['remuteQuietSeconds'],.5);self.assertIs(report['postPlayerGain'],True)
 def test_muted_os_output_cannot_pass_on_good_mixer_tap_claim(self):
  with self.assertRaises(smoke.SmokeFailure):self.assess(bytes(len(self.good)))
 def test_swapped_lanes_fail(self):
  data=b''.join(struct.pack('<ff',right,left) for left,right in struct.iter_unpack('<ff',self.good))
  with self.assertRaises(smoke.SmokeFailure):self.assess(data)
 def test_gap_in_required_contiguous_output_fails(self):
  data=bytearray(self.good);data[int(2*44100)*8:int(2.1*44100)*8]=bytes(int(.1*44100)*8)
  with self.assertRaises(smoke.SmokeFailure):self.assess(bytes(data))
 def test_audible_initial_mute_or_final_remute_fails(self):
  for start,end in [(0,.2),(4.5,5.4)]:
   data=bytearray(self.good)
   for i in range(int(start*44100),int(end*44100)):struct.pack_into('<ff',data,i*8,.1,.1)
   with self.subTest(start=start),self.assertRaises(smoke.SmokeFailure):self.assess(bytes(data))
 def test_wrong_tone_fails(self):
  data=b''.join(struct.pack('<ff',(.15*math.sin(math.tau*880*i/44100) if .4<=i/44100<4.2 else 0),right) for i,(_,right) in enumerate(struct.iter_unpack('<ff',self.good)))
  with self.assertRaises(smoke.SmokeFailure):self.assess(data)
 def test_nonfinite_or_partial_frame_fails(self):
  bad=bytearray(self.good);struct.pack_into('<f',bad,8000,float('nan'))
  for data in [bytes(bad),self.good[:-1]]:
   with self.assertRaises(smoke.SmokeFailure):self.assess(data)
 def test_pre_tap_missing_route_timing_or_metadata_never_substitutes_for_os_capture(self):
  for key in ['postPlayerGain','ownedAppStream','verified','physicalHardware','bytes','unmuteOffsetSeconds','remuteOffsetSeconds']:
   with self.subTest(key=key),self.assertRaises(smoke.SmokeFailure):self.assess(change=lambda c:c.pop(key))
 def test_invalid_nonfinite_marker_or_short_interval_fails(self):
  for field,value in [('unmuteOffsetSeconds',float('nan')),('remuteOffsetSeconds',float('inf')),('unmuteOffsetSeconds',True),('remuteOffsetSeconds',2.0)]:
   with self.assertRaises(smoke.SmokeFailure):self.assess(change=lambda c:c.__setitem__(field,value))
 def test_file_and_byte_bounds(self):
  with self.assertRaises(smoke.SmokeFailure):self.assess(b'')
  with self.assertRaises(smoke.SmokeFailure):self.assess(bytes(44100*2*4*10+8))
 def test_remute_requires_still_playing_progress_not_pause_only_silence(self):
  for key,value in [('remuteWhilePlaying',False),('remuteWhilePlaying',None),('remuteClockAdvanceSeconds',None),('remuteClockAdvanceSeconds',float('nan')),('remuteClockAdvanceSeconds',0),('remuteClockAdvanceSeconds',True)]:
   with self.subTest(key=key,value=value),self.assertRaises(smoke.SmokeFailure):self.assess(change=lambda c:c.__setitem__(key,value))
 def test_nonlinux_private_mode_rejected_before_child_run(self):
  with patch.object(smoke.sys,'platform','darwin'),patch.object(smoke.shutil,'which',return_value='node'),patch.object(pathlib.Path,'is_file',return_value=True),patch.object(smoke,'run') as run:
   with self.assertRaises(smoke.SmokeFailure):smoke.run_mcp_native_audio(pathlib.Path('/r'),'target',pathlib.Path('/d'),{},'local:fixture',private_monitor={'tempRoot':'/tmp/fake'})
   run.assert_not_called()
@unittest.skipUnless(smoke.sys.platform == "linux", "Linux-only process-group recorder cleanup")
class OwnedCheckerTimeoutTests(unittest.TestCase):
 def test_exited_leader_and_term_ignoring_descendant_escalate_before_reaping(self):
  owner=dict(pid=777,pgid=777,sid=777,uid=1000,start=100,state='Z')
  descendant=dict(owner,pid=778,state='S',start=101)
  phase={'value':0}; child=Mock(pid=777); clock=iter(range(100))
  def rows(_):return [owner,descendant] if phase['value']<2 else ([] if phase['value']==3 else [owner])
  def kill(pid,sig):
   self.assertEqual(pid,777);child.wait.assert_not_called()
   phase['value']=1 if sig==smoke.signal.SIGTERM else 2
  def reap(**kw):
   self.assertEqual(phase['value'],2);phase['value']=3
  child.wait.side_effect=reap
  with patch.object(smoke,'owned_group_members',side_effect=rows),patch.object(smoke,'linux_process_identity',return_value=owner),patch.object(smoke.os,'killpg',side_effect=kill) as signal_group,patch.object(smoke.time,'monotonic',side_effect=lambda:next(clock)),patch.object(smoke.time,'sleep'):
   self.assertTrue(smoke.finish_owned_checker_group(child,owner))
  self.assertEqual([c.args for c in signal_group.call_args_list],[(777,smoke.signal.SIGTERM),(777,smoke.signal.SIGKILL)])
  child.wait.assert_called_once_with(timeout=3)
 def test_reused_leader_never_signalled(self):
  owner=dict(pid=777,pgid=777,sid=777,uid=1000,start=100,state='S')
  with patch.object(smoke,'owned_group_members',return_value=[owner]),patch.object(smoke,'linux_process_identity',return_value=dict(owner,start=200)),patch.object(smoke.os,'killpg') as kill:
   with self.assertRaises(smoke.SmokeFailure):smoke.finish_owned_checker_group(Mock(),owner)
   kill.assert_not_called()
 def test_foreign_session_or_uid_is_rejected(self):
  owner=dict(pid=777,pgid=777,sid=777,uid=1000,start=100,state='S')
  for change in [{'sid':1},{'uid':2},{'start':99}]:
   with self.assertRaises(smoke.SmokeFailure):smoke.check_owned_group_rows(owner,[dict(owner,pid=778,**change)])
 def test_clean_group_reaps_without_signal(self):
  owner=dict(pid=777,pgid=777,sid=777,uid=1000,start=100,state='Z');child=Mock()
  with patch.object(smoke,'owned_group_members',return_value=[]),patch.object(smoke.os,'killpg') as kill:
   self.assertFalse(smoke.finish_owned_checker_group(child,owner));kill.assert_not_called();child.wait.assert_called_once()




if __name__ == "__main__":
    unittest.main()
