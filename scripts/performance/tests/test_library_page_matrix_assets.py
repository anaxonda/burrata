import importlib.util
import json
import math
import sqlite3
import tempfile
import unittest
from unittest.mock import patch
import wave
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / 'library_page_matrix.py'
SPEC = importlib.util.spec_from_file_location('library_page_matrix', SCRIPT)
MATRIX = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MATRIX)
MATRIX.ROOT = next(
    ancestor for ancestor in (Path(__file__).resolve(), *Path(__file__).resolve().parents)
    if (ancestor / 'src-tauri/src/schema.sql').is_file()
)
MATRIX.SCHEMA = MATRIX.ROOT / 'src-tauri/src/schema.sql'


class AssetMatrixFixtureTests(unittest.TestCase):
    def test_startup_only_cli_is_opt_in_and_never_waits_for_projection(self):
        parser = MATRIX.build_parser()
        common = ['--binary', 'fixture-binary', '--output', 'fixture-output']
        self.assertEqual(parser.parse_args(common).mode, 'matrix')
        self.assertEqual(parser.parse_args(common).grant_layout, 'external')
        args = parser.parse_args(common + ['--mode', 'startup-only'])
        self.assertEqual(args.mode, 'startup-only')
        self.assertEqual(args.projection_state, 'initial')
        with self.assertRaises(SystemExit):
            MATRIX.main(common + ['--mode', 'startup-only', '--projection-state', 'ready'])
        with self.assertRaises(SystemExit):
            MATRIX.main(common + ['--grant-layout', 'configured-root'])
        root_args = parser.parse_args(common + ['--fixture-mode', 'asset-backed', '--grant-layout', 'configured-root'])
        self.assertEqual(root_args.grant_layout, 'configured-root')

    def test_startup_only_branch_writes_registration_only_report_and_cleans_launcher(self):
        class FakeLauncher:
            def __init__(self, command, **kwargs):
                self.command = command
                self.poll_calls = 0
                self.signals = []
                self.waited = False
                kwargs['stdout'].write(b'MCP confirms the app is running\n')
                kwargs['stdout'].flush()

            def poll(self):
                self.poll_calls += 1
                return None if self.poll_calls <= 2 else 0

            def send_signal(self, sig):
                self.signals.append(sig)

            def wait(self, timeout=None):
                self.waited = True
                return 0

            def terminate(self):
                raise AssertionError('launcher should stop with SIGINT')

        with tempfile.TemporaryDirectory(prefix='burrata-startup-only-') as temp:
            temp_path = Path(temp)
            binary = temp_path / 'burrata'
            binary.write_bytes(b'isolated synthetic binary bytes')
            output = temp_path / 'output'
            output.mkdir()
            launcher_box = []

            def fake_popen(command, **kwargs):
                launcher = FakeLauncher(command, **kwargs)
                launcher_box.append(launcher)
                profile = Path(command[command.index('--isolated-profile') + 1])
                automation = profile / 'data/org.burrata.discovery/automation.json'
                automation.parent.mkdir(parents=True, exist_ok=True)
                automation.write_text('{}', encoding='utf-8')
                return launcher

            def fake_seed(profile, count, fixture_mode, grant_layout):
                music = profile / 'fixture/Music'
                music.mkdir(parents=True)
                (profile / 'data/org.burrata.discovery').mkdir(parents=True, exist_ok=True)
                return music

            with patch.object(MATRIX, 'seed', side_effect=fake_seed), \
                 patch.object(MATRIX.subprocess, 'Popen', side_effect=fake_popen), \
                 patch.object(MATRIX.subprocess, 'run') as page_mcp, \
                 patch.object(MATRIX, 'app_pid', side_effect=[43210, TimeoutError('stopped')]), \
                 patch.object(MATRIX, 'projection_state_for_run', side_effect=AssertionError('startup-only must skip projection wait')):
                result = MATRIX.run_count(binary, output, 1000, 100, 'catalog', 'initial', 20, 'startup-only')

            page_mcp.assert_not_called()
            self.assertTrue(launcher_box[0].waited)
            self.assertEqual(launcher_box[0].signals, [MATRIX.signal.SIGINT])
            report = json.loads(Path(result['report']).read_text(encoding='utf-8'))
            self.assertEqual(report['mode'], 'startup-only')
            self.assertEqual(report['count'], 1000)
            self.assertEqual(report['binarySha256'], MATRIX.hashlib.sha256(binary.read_bytes()).hexdigest())
            self.assertTrue(math.isfinite(report['startupWaitMs']))
            self.assertEqual(report['timedPageReads'], 0)
            self.assertEqual(report['pageMatrixCoverage'], 'none')
            self.assertFalse(report['projectionReadinessWaited'])
            self.assertEqual(report['harnessProviderRequests'], 0)
            profile = Path(launcher_box[0].command[launcher_box[0].command.index('--isolated-profile') + 1])
            import shutil
            shutil.rmtree(profile)

    def test_startup_registration_timeout_is_bounded_and_keeps_default(self):
        self.assertEqual(MATRIX.bounded_startup_timeout('20'), 20)
        self.assertEqual(MATRIX.bounded_startup_timeout('120'), 120)
        for invalid in ('0', '181', '-1', '20.5', 'slow'):
            with self.subTest(invalid=invalid), self.assertRaises(MATRIX.argparse.ArgumentTypeError):
                MATRIX.bounded_startup_timeout(invalid)

    def test_launcher_registration_timeout_validation_rejects_overflow_and_expressions(self):
        launcher = (MATRIX.ROOT / 'scripts/dev/launch-current.sh').read_text()
        validation = launcher.split('# Protected mode must own its Vite server;', 1)[0]
        for value in ('1', '20', '180', '0', '181', '08', '1+1', '999999999999999999999999999'):
            with self.subTest(value=value):
                result = MATRIX.subprocess.run(['bash', '-c', validation],
                    env=dict(MATRIX.os.environ, BURRATA_AUTOMATION_REGISTRATION_TIMEOUT_SECONDS=value),
                    capture_output=True, timeout=2)
                self.assertEqual(result.returncode, 0 if value in ('1', '20', '180') else 2)

    def test_seed_quota_failure_is_retained_without_launching_app_or_leaking_error(self):
        with tempfile.TemporaryDirectory(prefix='burrata-matrix-seed-failure-') as temp:
            error = OSError(MATRIX.errno.EDQUOT, 'private fixture path and secret')
            with patch.object(MATRIX, 'seed', side_effect=error) as seed, patch.object(MATRIX.subprocess, 'Popen') as launch:
                try:
                    with self.assertRaisesRegex(RuntimeError, 'finite failure artifact retained'):
                        MATRIX.run_count(Path('/unused'), Path(temp), 1000, 100, 'asset-backed')
                    launch.assert_not_called()
                    text = (Path(temp) / 'actual-mcp-page-matrix-1000-asset-backed-failure.json').read_text()
                    result = json.loads(text)
                    self.assertEqual(result['failureCategory'], 'fixture_storage_quota')
                    self.assertTrue(result['failedBeforeAppLaunch'])
                    self.assertEqual(result['osErrorNumber'], MATRIX.errno.EDQUOT)
                    self.assertNotIn('secret', text)
                    self.assertNotIn('private fixture', text)
                finally:
                    if seed.call_args:
                        seed.call_args.args[0].rmdir()

    def test_projection_gate_is_opt_in_and_separately_labeled(self):
        with patch.object(MATRIX, 'wait_for_asset_projection_ready', side_effect=AssertionError('initial must not wait')):
            initial = MATRIX.projection_state_for_run(Path('/unused'), 8, 'asset-backed', 'initial')
            catalog = MATRIX.projection_state_for_run(Path('/unused'), 8, 'catalog', 'ready')
        self.assertEqual(initial['mode'], 'initial')
        self.assertFalse(initial['waited'])
        self.assertIn('startup/fallback latency is included', initial['note'])
        self.assertEqual(catalog['mode'], 'initial')
        with patch.object(MATRIX, 'wait_for_asset_projection_ready', return_value={'ready': True, 'waitMs': 25}) as wait:
            ready = MATRIX.projection_state_for_run(Path('/synthetic'), 8, 'asset-backed', 'ready')
        wait.assert_called_once_with(Path('/synthetic'), 8)
        self.assertEqual(ready, {'mode': 'ready', 'waited': True, 'ready': True, 'waitMs': 25})

    def test_asset_backed_fixture_uses_unique_paths_and_exact_or_stale_analysis(self):
        with tempfile.TemporaryDirectory(prefix='burrata-matrix-fixture-') as temp:
            profile = Path(temp)
            music = MATRIX.seed(profile, 50, 'asset-backed')
            db = sqlite3.connect(profile / 'data/org.burrata.discovery/burrata.sqlite')
            try:
                rows = db.execute('SELECT id,path,hash,payload FROM local_assets ORDER BY id').fetchall()
                self.assertEqual(db.execute('PRAGMA user_version').fetchone()[0], 3)
                self.assertEqual(db.execute("SELECT COUNT(*) FROM settings WHERE key='application'").fetchone()[0], 0)
                migrated = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
                self.assertTrue({'browser_sources', 'browser_imports', 'provider_cache', 'artist_mappings', 'asset_analyses'}.issubset(migrated))
                self.assertEqual(len(rows), 50)
                self.assertEqual(len({row[0] for row in rows}), 50)
                self.assertEqual(len({row[1] for row in rows}), 50)
                self.assertTrue(all(Path(path).is_file() and Path(path).parent == music for _, path, _, _ in rows))
                self.assertEqual(len({hash_value for _, _, hash_value, _ in rows}), 1)
                with wave.open(rows[0][1], 'rb') as wav:
                    self.assertEqual((wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getnframes()), (1, 2, 8000, 2))
                self.assertEqual(len({Path(path).stat().st_ino for _, path, _, _ in rows}), 1)

                exact = db.execute("SELECT COUNT(*) FROM asset_analyses WHERE file_hash=(SELECT hash FROM local_assets WHERE id=asset_id)").fetchone()[0]
                stale = db.execute("SELECT COUNT(*) FROM asset_analyses WHERE file_hash NOT IN (SELECT hash FROM local_assets WHERE id=asset_id)").fetchone()[0]
                self.assertEqual(exact, 50)  # current-version completion suppresses startup reanalysis
                self.assertEqual(stale, 4)  # indices 7, 21, 35, 49
                current_version = MATRIX.current_analysis_version()
                self.assertEqual(db.execute("SELECT COUNT(*) FROM asset_analyses WHERE version=? AND file_hash=(SELECT hash FROM local_assets WHERE id=asset_id)", (current_version,)).fetchone()[0], 50)
                self.assertEqual(db.execute("SELECT COUNT(*) FROM asset_analyses WHERE version=? AND file_hash=(SELECT hash FROM local_assets WHERE id=asset_id) AND json_type(payload,'$.bpm') IS NOT NULL", (current_version,)).fetchone()[0], 4)
                self.assertEqual(db.execute("SELECT COUNT(*) FROM asset_analyses WHERE file_hash!= (SELECT hash FROM local_assets WHERE id=asset_id) AND version=?", (current_version,)).fetchone()[0], 4)
                eligible = db.execute("SELECT COUNT(*) FROM local_assets a WHERE COALESCE(json_extract(a.payload,'$.processing'),'')<>'external-readonly' AND NOT EXISTS (SELECT 1 FROM asset_analyses x WHERE x.asset_id=a.id AND x.file_hash=a.hash AND x.version=? AND x.status='completed')", (current_version,)).fetchone()[0]
                self.assertEqual(eligible, 0)
                self.assertEqual(db.execute("SELECT COUNT(*) FROM recordings WHERE json_extract(manual_metadata,'$.bpm') IS NOT NULL").fetchone()[0], 1)
                self.assertEqual(db.execute("SELECT COUNT(*) FROM recordings WHERE json_extract(manual_metadata,'$.title') IS NOT NULL").fetchone()[0], 0)
                asset_track = json.loads(rows[7][3])['track']
                self.assertEqual(asset_track['localAvailability'], 'available')
                self.assertEqual(asset_track['assetId'], rows[7][0])
                self.assertIsNone(asset_track['bpm'])
                self.assertEqual(db.execute("SELECT json_extract(payload,'$.bpm') FROM asset_analyses WHERE asset_id='matrix-asset-000007' AND file_hash LIKE 'stale-%' AND version=?", (current_version,)).fetchone()[0], 199.0)
                unknown = db.execute("SELECT l.metadata,a.payload FROM catalog_listings l JOIN local_assets a ON a.recording_id=l.recording_id WHERE l.id='fixture:matrix-000000'").fetchone()
                self.assertIsNone(json.loads(unknown[0])['durationSeconds'])
                self.assertIsNone(json.loads(unknown[1])['durationSeconds'])
                self.assertIsNone(json.loads(unknown[1])['track']['durationSeconds'])
            finally:
                db.close()

    def test_configured_root_fixture_persists_exact_isolated_music_directory(self):
        with tempfile.TemporaryDirectory(prefix='burrata-matrix-configured-root-') as temp:
            profile = Path(temp)
            music = MATRIX.seed(profile, 12, 'asset-backed', 'configured-root')
            db = sqlite3.connect(profile / 'data/org.burrata.discovery/burrata.sqlite')
            try:
                raw = db.execute("SELECT value FROM settings WHERE key='application'").fetchone()
                self.assertIsNotNone(raw)
                settings = json.loads(raw[0])
                self.assertEqual(settings['musicDirectory'], str(music))
                self.assertEqual(settings['playlistDirectories'], [])
                self.assertEqual(db.execute('SELECT COUNT(*) FROM local_assets').fetchone()[0], 12)
                paths = [row[0] for row in db.execute('SELECT path FROM local_assets ORDER BY path')]
                self.assertEqual(len(paths), 12)
                self.assertTrue(all(Path(path).is_file() and Path(path).parent == music for path in paths))
                binary = profile / 'synthetic-burrata'
                binary.write_bytes(b'synthetic fixture executable')
                report = MATRIX.startup_only_report(binary, 12, 'asset-backed',
                                                    123.0, 20, 4321, profile, 'configured-root')
                self.assertEqual(report['grantLayout'], 'configured-root')
                self.assertEqual(report['fixture']['fileCount'], 12)
            finally:
                db.close()

    def test_default_catalog_fixture_remains_asset_free(self):
        with tempfile.TemporaryDirectory(prefix='burrata-matrix-catalog-') as temp:
            profile = Path(temp)
            music = MATRIX.seed(profile, 20)
            db = sqlite3.connect(profile / 'data/org.burrata.discovery/burrata.sqlite')
            try:
                self.assertEqual(db.execute('SELECT COUNT(*) FROM catalog_listings').fetchone()[0], 20)
                self.assertEqual(db.execute('SELECT COUNT(*) FROM local_assets').fetchone()[0], 0)
                self.assertEqual(db.execute('PRAGMA user_version').fetchone()[0], 1)
                self.assertEqual(db.execute("SELECT COUNT(*) FROM settings WHERE key='application'").fetchone()[0], 0)
                self.assertFalse(list(music.iterdir()))
            finally:
                db.close()

    def test_projection_gate_waits_for_every_single_asset_listing(self):
        with tempfile.TemporaryDirectory(prefix='burrata-matrix-readiness-') as temp:
            profile = Path(temp)
            MATRIX.seed(profile, 8, 'asset-backed')
            db = sqlite3.connect(profile / 'data/org.burrata.discovery/burrata.sqlite')
            try:
                db.executescript('ALTER TABLE catalog_listings ADD COLUMN in_library INTEGER NOT NULL DEFAULT 1; ALTER TABLE catalog_listings ADD COLUMN sort_metadata_projection TEXT; ALTER TABLE catalog_listings ADD COLUMN sort_projection_revision INTEGER;')
                db.execute('PRAGMA user_version=9')
                db.commit()
                state = MATRIX.asset_projection_state(profile, 8)
                self.assertFalse(state['ready'])
                self.assertEqual(state['pendingSingleAssetProjections'], 8)
                db.execute('UPDATE catalog_listings SET sort_metadata_projection="{}",sort_projection_revision=?', (state['revision'],))
                db.commit()
                state = MATRIX.asset_projection_state(profile, 8)
                self.assertTrue(state['ready'])
                self.assertEqual(state['pendingSingleAssetProjections'], 0)
            finally:
                db.close()


if __name__ == '__main__':
    unittest.main()
