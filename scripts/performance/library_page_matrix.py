#!/usr/bin/env python3
"""Benchmark real Burrata MCP library paging over isolated synthetic fixtures.

Usage after a current immutable debug build is frozen:
  python scripts/performance/library_page_matrix.py \
    --binary .cache/live-playback-<sha>/burrata \
    --output docs/performance-results/<date>-library-page-matrix

Each count gets a fresh `/tmp/burrata-launch.*` profile and isolated Music root;
only page reads run, with no playback/provider network requests. Output JSON reports
real MCP wall latency, executable-process CPU ticks, serialized result bytes, exact
identity/order parity, and retained redacted failure categories.
"""
import argparse
import errno
import json
import os
import hashlib
import re
import signal
import sqlite3
import subprocess
import sys
import time
import uuid
import wave
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / 'scripts/dev/launch-current.sh'
SCHEMA = ROOT / 'src-tauri/src/schema.sql'
MATRIX = ROOT / 'scripts/performance/library_page_matrix.mjs'
MARKER = 'burrata isolated memory run\n'
COUNTS = (1000, 5000, 20000, 50000)
FIXTURE_MODES = ('catalog', 'asset-backed')
GRANT_LAYOUTS = ('external', 'configured-root')


def current_analysis_version():
    media_source = (ROOT / 'src-tauri/src/media.rs').read_text(encoding='utf-8')
    match = re.search(r'pub const ANALYSIS_VERSION: &str = "([^"]+)";', media_source)
    if not match:
        raise RuntimeError('could not read pinned ANALYSIS_VERSION from media.rs')
    return match.group(1)


def fixture_suffix(fixture_mode: str, grant_layout: str) -> str:
    suffix = '-asset-backed' if fixture_mode == 'asset-backed' else ''
    return suffix + ('-configured-root' if grant_layout == 'configured-root' else '')


def advance_asset_fixture_to_schema_v3(db: sqlite3.Connection):
    """Apply Store::open migrations 2 and 3 before appending asset rows.

    The base schema is version 1. Setting the version without these tables
    makes Store::open attempt the migrations again, so keep the SQL aligned
    with storage.rs rather than pre-creating only asset_analyses.
    """
    db.executescript("""
        BEGIN IMMEDIATE;
        CREATE TABLE browser_sources(source_url TEXT PRIMARY KEY, provider TEXT NOT NULL,
            kind TEXT NOT NULL, collection_id TEXT NOT NULL REFERENCES collections(id),
            latest_snapshot_id TEXT NOT NULL REFERENCES chart_snapshots(id));
        CREATE TABLE browser_imports(snapshot_id TEXT PRIMARY KEY REFERENCES chart_snapshots(id),
            payload TEXT NOT NULL);
        CREATE TABLE provider_cache(namespace TEXT NOT NULL,key TEXT NOT NULL,payload TEXT NOT NULL,
            expires_at TEXT NOT NULL,PRIMARY KEY(namespace,key));
        CREATE TABLE artist_mappings(provider TEXT NOT NULL,provider_artist_id TEXT NOT NULL,
            artist_id TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(provider,provider_artist_id));
        CREATE TABLE asset_analyses(asset_id TEXT NOT NULL REFERENCES local_assets(id),file_hash TEXT NOT NULL,
            version TEXT NOT NULL,status TEXT NOT NULL,payload TEXT NOT NULL,updated_at TEXT NOT NULL,
            PRIMARY KEY(asset_id,file_hash,version));
        PRAGMA user_version=3;
        COMMIT;
    """)


def seed(profile: Path, count: int, fixture_mode: str = 'catalog', grant_layout: str = 'external'):
    if fixture_mode not in FIXTURE_MODES:
        raise ValueError('unknown fixture mode')
    if grant_layout not in GRANT_LAYOUTS or (grant_layout == 'configured-root' and fixture_mode != 'asset-backed'):
        raise ValueError('configured-root grant layout requires the asset-backed fixture')
    data = profile / 'data/org.burrata.discovery'
    data.mkdir(parents=True)
    (profile / '.burrata-isolated-profile').write_text(MARKER, encoding='utf-8')
    music = profile / 'fixture/Music'
    music.mkdir(parents=True)
    asset_master = None
    asset_hash = None
    analysis_version = None
    if fixture_mode == 'asset-backed':
        asset_master = profile / 'fixture/tiny-matrix.wav'
        with wave.open(str(asset_master), 'wb') as wav:
            wav.setnchannels(1)
            wav.setsampwidth(2)
            wav.setframerate(8000)
            wav.writeframes(b'\x00\x00\x01\x00')
        asset_hash = hashlib.sha256(asset_master.read_bytes()).hexdigest()
        analysis_version = current_analysis_version()

    db = sqlite3.connect(data / 'burrata.sqlite')
    db.executescript(SCHEMA.read_text(encoding='utf-8'))
    if fixture_mode == 'asset-backed':
        advance_asset_fixture_to_schema_v3(db)
    if grant_layout == 'configured-root':
        # Store::settings reads this exact application preference before startup
        # asset grants. It is synthetic and points only at this isolated fixture.
        settings = {
            'musicDirectory': str(music), 'autoPreview': False,
            'pauseOnSourceOpen': False, 'sourceOpenAtPosition': False,
            'previewDelayMs': 250, 'theme': 'dark', 'ytDlpPath': 'yt-dlp',
            'ffmpegPath': 'ffmpeg', 'ffprobePath': 'ffprobe', 'jsRuntimePath': 'node',
            'preferredDownloadSource': 'youtube', 'preferredStreamingSource': 'youtube',
            'overridePlaylistTrackUrls': False, 'streamingFallbackEnabled': True,
            'streamingFallbackDelayMs': 1500, 'streamingFallbackSources': ['youtube', 'soundcloud'],
            'downloadFallbackEnabled': False,
            'downloadFallbackSources': ['monochrome', 'slskd', 'youtube', 'soundcloud'],
            'downloadFallbackAllowLossy': False, 'playlistDirectories': [],
        }
        db.execute('INSERT INTO settings(key,value) VALUES(\'application\',?)',
                   (json.dumps(settings, separators=(',', ':')),))
    now = datetime.now(timezone.utc).isoformat()
    try:
        with db:
            for i in range(count):
                track_id = f'fixture:matrix-{i:06d}'
                work_id = f'work:{track_id}'
                title = f'Library Track {i//10:05d}'
                artist = f'Artist Group {i%23:02d}'
                version = 'ΑΘΉΝΑ Marker' if i % 997 == 777 else 'Original'
                duration = None if i == 0 else 180.0 + i // 10
                track = {
                    'id': track_id, 'provider': 'fixture', 'providerId': f'matrix-{i:06d}',
                    'sourceUrl': '', 'artist': artist, 'title': title, 'version': version,
                    'album': 'Synthetic Matrix', 'albumArtist': None, 'recordingDate': None,
                    'trackNumber': None, 'trackTotal': None, 'discNumber': None, 'discTotal': None,
                    'remixers': [], 'label': None, 'genre': None, 'normalizedGenre': None,
                    'durationSeconds': duration, 'bpm': None, 'key': None, 'releaseDate': None,
                    'artworkUrl': None, 'isrc': None, 'catalogNumber': None, 'rank': None,
                    'previewUrl': None, 'favorite': False, 'disposition': None,
                    'localPath': None, 'localAvailability': 'none', 'assetId': None,
                    'provenance': {'synthetic': True},
                }
                manual = {}
                if fixture_mode == 'asset-backed':
                    if i % 997 == 777:
                        manual['title'] = f'Z Manual Override {i:06d}'
                    if i % 211 == 0:
                        manual['bpm'] = 180.0 + i % 13
                metadata = json.dumps(track, separators=(',', ':'), ensure_ascii=False)
                manual_metadata = json.dumps(manual, separators=(',', ':'), ensure_ascii=False)
                db.execute('INSERT INTO works(id,title,artist) VALUES(?,?,?)', (work_id, title, artist))
                db.execute('INSERT INTO recordings(id,work_id,version,manual_metadata) VALUES(?,?,?,?)', (track_id, work_id, version, manual_metadata))
                db.execute('INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?,?,?,?,?,?,?)',
                           (track_id, 'fixture', f'matrix-{i:06d}', track_id, '', metadata, now))
                if fixture_mode == 'asset-backed':
                    asset_id = f'matrix-asset-{i:06d}'
                    path = music / f'asset-{i:06d}.wav'
                    os.link(asset_master, path)
                    asset_track = {**track, 'localPath': str(path), 'localAvailability': 'available', 'assetId': asset_id,
                                   'provenance': {'synthetic': True, 'localAudioFactsHash': asset_hash}}
                    asset_track['bpm'] = None if i % 7 == 0 else 60.0 + i % 43
                    asset = {
                        'id': asset_id, 'recordingId': track_id, 'path': str(path), 'hash': asset_hash,
                        'sourceCodec': 'wav', 'finalCodec': 'wav', 'sourceBitrate': None, 'finalBitrate': None,
                        # Duration metadata is deliberately omitted from the asset
                        # payload; the tiny fixture WAV's physical duration is not
                        # representative of the synthetic catalog recording.
                        'sampleRate': 8000, 'channels': 1, 'bitDepth': 16, 'durationSeconds': None,
                        'sizeBytes': asset_master.stat().st_size, 'bitrateOrigin': 'file-size-estimate',
                        'processing': 'fixture-hardlink', 'track': asset_track,
                    }
                    db.execute('INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES(?,?,?,?,?)',
                               (asset_id, track_id, str(path), asset_hash,
                                json.dumps(asset, separators=(',', ':'), ensure_ascii=False)))
                    # Seed a completed analysis row at the app's current version
                    # for every asset. This makes the fixture steady-state and
                    # prevents startup analysis from adding failures while timing.
                    analysis = {'bpm': 70.0 + i % 37} if i % 14 == 0 else {}
                    analysis_hash = asset_hash
                    db.execute('INSERT INTO asset_analyses(asset_id,file_hash,version,status,payload,updated_at) VALUES(?,?,?,?,?,?)',
                               (asset_id, analysis_hash, analysis_version, 'completed',
                                json.dumps(analysis, separators=(',', ':')), now))
                    if i % 14 != 0 and i % 7 == 0:
                        # Stale-hash analysis must not make missing asset BPM sortable.
                        analysis_hash = 'stale-' + asset_hash
                        analysis = {'bpm': 199.0, 'provenance': {'method': 'synthetic-stale-analysis'}}
                        db.execute('INSERT INTO asset_analyses(asset_id,file_hash,version,status,payload,updated_at) VALUES(?,?,?,?,?,?)',
                                   (asset_id, analysis_hash, analysis_version, 'completed',
                                    json.dumps(analysis, separators=(',', ':')), now))
    finally:
        db.close()
    return music


def sort_projection_revision():
    source = (ROOT / 'src-tauri/src/track_sort_sql.rs').read_text(encoding='utf-8')
    match = re.search(r'pub const AVAILABILITY_MASK_REVISION: i64 = (\d+);', source)
    if not match:
        raise RuntimeError('could not read AVAILABILITY_MASK_REVISION from track_sort_sql.rs')
    return int(match.group(1))


def asset_projection_state(profile: Path, expected_count: int):
    """Read exact-cache readiness without calling the latency-sensitive MCP fallback."""
    db_path = profile / 'data/org.burrata.discovery/burrata.sqlite'
    revision = sort_projection_revision()
    uri = f'file:{db_path.as_posix()}?mode=ro'
    with sqlite3.connect(uri, uri=True, timeout=2) as db:
        version = db.execute('PRAGMA user_version').fetchone()[0]
        columns = {row[1] for row in db.execute("PRAGMA table_info('catalog_listings')")}
        required = {'in_library', 'sort_metadata_projection', 'sort_projection_revision'}
        if version < 9 or not required.issubset(columns):
            return {'ready': False, 'reason': 'schema_migration_pending', 'userVersion': version}
        counts = db.execute("""
            SELECT COUNT(*),
                   SUM(asset_count),
                   SUM(CASE WHEN asset_count=1 THEN 1 ELSE 0 END),
                   SUM(CASE WHEN asset_count=1 AND
                       (sort_metadata_projection IS NULL OR sort_projection_revision IS NOT ?)
                       THEN 1 ELSE 0 END)
            FROM (
                SELECT l.id,l.sort_metadata_projection,l.sort_projection_revision,
                       COUNT(a.id) AS asset_count
                FROM catalog_listings l
                JOIN recordings r ON r.id=l.recording_id
                LEFT JOIN local_assets a ON a.recording_id=r.id
                WHERE l.id=l.recording_id AND l.in_library=1
                GROUP BY l.id
            )
        """, (revision,)).fetchone()
    listings, assets, single_asset_listings, pending = (int(v or 0) for v in counts)
    if listings != expected_count or assets != expected_count or single_asset_listings != expected_count:
        raise RuntimeError('asset fixture is not the expected one-asset-per-listing library')
    return {'ready': pending == 0, 'revision': revision, 'libraryListings': listings,
            'assets': assets, 'singleAssetListings': single_asset_listings,
            'pendingSingleAssetProjections': pending}


def wait_for_asset_projection_ready(profile: Path, expected_count: int, timeout=180):
    started = time.monotonic()
    last_state = None
    while time.monotonic() - started < timeout:
        last_state = asset_projection_state(profile, expected_count)
        if last_state['ready']:
            last_state['waitMs'] = round((time.monotonic() - started) * 1000, 1)
            return last_state
        time.sleep(.25)
    raise TimeoutError(f'asset sort projection readiness timed out: {last_state}')


def projection_state_for_run(profile: Path, expected_count: int, fixture_mode: str, projection_state: str):
    """Keep startup timing as default; ready mode explicitly waits outside samples."""
    if projection_state == 'ready' and fixture_mode == 'asset-backed':
        state = wait_for_asset_projection_ready(profile, expected_count)
        return {'mode': 'ready', 'waited': True, **state}
    return {'mode': 'initial', 'waited': False,
            'note': 'no projection readiness gate; startup/fallback latency is included'}


def app_pid(binary: Path, profile: Path, timeout=20):
    deadline = time.monotonic() + timeout
    wanted = binary.resolve()
    expected_config = f'XDG_CONFIG_HOME={profile}/config'.encode()
    expected_data = f'XDG_DATA_HOME={profile}/data'.encode()
    while time.monotonic() < deadline:
        found = []
        for entry in Path('/proc').iterdir():
            if not entry.name.isdigit():
                continue
            try:
                if Path(entry / 'exe').resolve() != wanted:
                    continue
                env = set((entry / 'environ').read_bytes().split(b'\0'))
                if expected_config in env and expected_data in env:
                    found.append(int(entry.name))
            except OSError:
                pass
        if len(found) == 1:
            return found[0]
        if len(found) > 1:
            raise RuntimeError('multiple exact binary/profile processes found')
        time.sleep(.2)
    raise TimeoutError('isolated app process was not found')


def bounded_startup_timeout(value: str) -> int:
    try:
        seconds = int(value)
    except (TypeError, ValueError):
        raise argparse.ArgumentTypeError('must be an integer from 1 to 180 seconds')
    if not 1 <= seconds <= 180:
        raise argparse.ArgumentTypeError('must be an integer from 1 to 180 seconds')
    return seconds


def startup_only_report(binary: Path, count: int, fixture_mode: str, startup_wait_ms: float,
                        startup_timeout_seconds: int, pid: int, profile: Path,
                        grant_layout: str = 'external'):
    """Report only the bounded isolated-launch handshake; this is not page coverage."""
    return {
        'schemaVersion': 1,
        'workload': 'burrata_isolated_startup_only',
        'mode': 'startup-only',
        'count': count,
        'fixtureMode': fixture_mode,
        'grantLayout': grant_layout,
        'profileFixture': 'fresh isolated synthetic only',
        'binarySha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'startupWaitMs': startup_wait_ms,
        'startupRegistrationTimeoutSeconds': startup_timeout_seconds,
        'exactBinaryProfileProcessVerified': True,
        'appPid': pid,
        'profilePath': str(profile),
        'timedPageReads': 0,
        'pageMatrixCoverage': 'none',
        'projectionReadinessWaited': False,
        'harnessProviderRequests': 0,
        'limitations': [
            'Measures fresh isolated app/MCP/frontend registration only.',
            'The harness performs no page MCP calls or provider requests; timed reads and sort coverage are absent.',
        ],
        'fixtureSchemaVersion': 3 if fixture_mode == 'asset-backed' else 1,
        'fixtureSchemaProvenance': ('schema.sql v1 plus Store::open v1→v2→v3 table migrations' if fixture_mode == 'asset-backed' else 'schema.sql v1; Store::open performs normal app migrations'),
        **({'fixture': {'kind': 'synthetic tiny WAV local_assets', 'fileCount': count,
                        'audioQualification': False}}
           if fixture_mode == 'asset-backed' else {}),
    }


def run_count(binary: Path, output: Path, count: int, tick_hz: int, fixture_mode: str = 'catalog', projection_state: str = 'initial', startup_timeout_seconds: int = 20, run_mode: str = 'matrix', grant_layout: str = 'external'):
    if run_mode not in ('matrix', 'startup-only'):
        raise ValueError('unknown run mode')
    if grant_layout not in GRANT_LAYOUTS or (grant_layout == 'configured-root' and fixture_mode != 'asset-backed'):
        raise ValueError('configured-root grant layout requires the asset-backed fixture')
    name = f'Matrix{datetime.now().strftime("%Y%m%d%H%M%S")}{count}{uuid.uuid4().hex[:6]}'
    profile = Path('/tmp') / f'burrata-launch.{name}'
    try:
        profile.mkdir(mode=0o700)
        music = seed(profile, count, fixture_mode, grant_layout)
    except Exception as error:
        os_code = error.errno if isinstance(error, OSError) else None
        sql_code = getattr(error, 'sqlite_errorcode', None)
        category = ('fixture_storage_quota' if os_code == errno.EDQUOT else
                    'fixture_storage_exhausted' if os_code == errno.ENOSPC or sql_code == sqlite3.SQLITE_FULL else
                    'fixture_setup_failed')
        failure = {'schemaVersion': 1,
                   'workload': ('burrata_isolated_startup_only' if run_mode == 'startup-only' else 'actual_burrata_mcp_library_page_matrix'),
                   'mode': run_mode, 'count': count, 'fixtureMode': fixture_mode, 'grantLayout': grant_layout, 'failureCount': 1,
                   'failureCategory': category, 'phase': 'fixture_seed',
                   'failedBeforeAppLaunch': True, 'profileRetained': profile.exists(),
                   'osErrorNumber': os_code if isinstance(os_code, int) else None,
                   'sqliteErrorCode': sql_code if isinstance(sql_code, int) else None}
        suffix = fixture_suffix(fixture_mode, grant_layout)
        failure_name = (f'actual-mcp-startup-only-{count}{suffix}-failure.json'
                        if run_mode == 'startup-only' else
                        f'actual-mcp-page-matrix-{count}{suffix}-failure.json')
        if run_mode == 'startup-only':
            failure.update({'timedPageReads': 0, 'pageMatrixCoverage': 'none'})
        (output / failure_name).write_text(json.dumps(failure, indent=2) + '\n')
        raise RuntimeError('isolated fixture setup failed; finite failure artifact retained') from None
    env = dict(os.environ, BURRATA_APP_BINARY=str(binary),
               BURRATA_ISOLATED_MUSIC_ROOT=str(music), BURRATA_PLAYBACK_ENGINE='native',
               BURRATA_PLAYBACK_TRACE='0',
               BURRATA_AUTOMATION_REGISTRATION_TIMEOUT_SECONDS=str(startup_timeout_seconds))
    launch_log = Path('/tmp') / f'library-page-launch-{count}-{uuid.uuid4().hex[:8]}.log'
    startup_started = time.monotonic()
    startup_wait_ms = None
    with launch_log.open('wb') as log:
        launcher = subprocess.Popen([str(LAUNCHER), '--isolated-profile', str(profile), '--keep-profile'],
                                    cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT,
                                    start_new_session=True)
        try:
            completed = None
            deadline = time.monotonic() + startup_timeout_seconds + 60
            discovery = profile / 'data/org.burrata.discovery/automation.json'
            while time.monotonic() < deadline:
                if launcher.poll() is not None:
                    raise RuntimeError(f'isolated launcher exited with code {launcher.returncode}')
                log.flush()
                if discovery.exists() and discovery.stat().st_size > 0:
                    # Launcher confirms authenticated MCP + mounted frontend before blocking.
                    if b'MCP confirms the app is running' in launch_log.read_bytes():
                        break
                time.sleep(.25)
            else:
                raise TimeoutError('isolated launcher handshake did not complete')
            startup_wait_ms = round((time.monotonic() - startup_started) * 1000, 1)
            pid = app_pid(binary, profile, timeout=startup_timeout_seconds + 10)
            suffix = fixture_suffix(fixture_mode, grant_layout)
            if run_mode == 'startup-only':
                out_file = output / f'actual-mcp-startup-only-{count}{suffix}.json'
                report = startup_only_report(binary, count, fixture_mode, startup_wait_ms,
                                             startup_timeout_seconds, pid, profile, grant_layout)
                if fixture_mode == 'asset-backed':
                    report['fixture']['analysisVersion'] = current_analysis_version()
                out_file.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
                launch_log.unlink(missing_ok=True)
                return {'count': count, 'fixtureMode': fixture_mode, 'mode': run_mode,
                        'report': str(out_file), 'exactBinaryProfileProcessVerified': True}

            projection_report = projection_state_for_run(profile, count, fixture_mode, projection_state)
            out_file = output / f'actual-mcp-page-matrix-{count}{suffix}.json'
            env['BURRATA_AUTOMATION_FILE'] = str(discovery)
            if projection_report['mode'] == 'ready':
                env['BURRATA_SORT_PROJECTION_READY'] = '1'
            completed = subprocess.run(['node', str(MATRIX), str(discovery), str(count), str(out_file), str(pid), str(tick_hz), fixture_mode, 'matrix', projection_state],
                                       cwd=ROOT, env=env, capture_output=True, text=True, timeout=900)
            if completed.returncode:
                if out_file.exists():
                    report = json.loads(out_file.read_text(encoding='utf-8'))
                    report['processExitCode'] = completed.returncode
                    out_file.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
                else:
                    out_file.write_text(json.dumps({'schemaVersion':1,'workload':'actual_burrata_mcp_library_page_matrix','count':count,'failureCount':1,'failureCategory':'mcp_process_failed','processExitCode':completed.returncode},indent=2)+'\n',encoding='utf-8')
                raise RuntimeError('MCP matrix validation failed; sanitized failure artifact retained')
            report = json.loads(out_file.read_text(encoding='utf-8'))
            report['binarySha256'] = __import__('hashlib').sha256(binary.read_bytes()).hexdigest()
            report['profileFixture'] = 'isolated synthetic only'
            report['fixtureMode'] = fixture_mode
            report['grantLayout'] = grant_layout
            report['projectionState'] = projection_report
            report['startupWaitMs'] = startup_wait_ms
            report['startupRegistrationTimeoutSeconds'] = startup_timeout_seconds
            report['fixtureSchemaVersion'] = 3 if fixture_mode == 'asset-backed' else 1
            report['fixtureSchemaProvenance'] = ('schema.sql v1 plus Store::open v1→v2→v3 table migrations' if fixture_mode == 'asset-backed' else 'schema.sql v1; Store::open performs normal app migrations')
            if fixture_mode == 'asset-backed':
                report['fixture']['analysisVersion'] = current_analysis_version()
            report['exactBinaryProfileProcessVerified'] = True
            out_file.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
            launch_log.unlink(missing_ok=True)
            return {'count': count, 'fixtureMode': fixture_mode, 'report': str(out_file), 'exactBinaryProfileProcessVerified': True}
        except Exception as error:
            log_text = launch_log.read_text(encoding='utf-8', errors='replace') if launch_log.exists() else ''
            category = ('mcp_registration_timeout' if 'did not register a fresh automation endpoint' in log_text else
                        'launcher_startup_failed' if 'launcher exited' in str(error) else
                        'mcp_registration_timeout' if 'handshake' in str(error) else
                        'isolated_app_identity_failed' if 'process' in str(error) else
                        'matrix_timeout' if isinstance(error, subprocess.TimeoutExpired) else
                        'isolated_matrix_setup_or_read_failed')
            failure = {'schemaVersion':1,
                       'workload': ('burrata_isolated_startup_only' if run_mode == 'startup-only' else 'actual_burrata_mcp_library_page_matrix'),
                       'count':count, 'grantLayout':grant_layout, 'failureCount':1,'failureCategory':category,
                       'startupWaitMs':startup_wait_ms if startup_wait_ms is not None else round((time.monotonic()-startup_started)*1000,1),
                       'startupRegistrationTimeoutSeconds':startup_timeout_seconds}
            if completed is not None and completed.returncode is not None:
                failure['processExitCode'] = completed.returncode
            suffix = fixture_suffix(fixture_mode, grant_layout)
            failure_name = (f'actual-mcp-startup-only-{count}{suffix}-failure.json'
                            if run_mode == 'startup-only' else
                            f'actual-mcp-page-matrix-{count}{suffix}-failure.json')
            failure['mode'] = run_mode
            if run_mode == 'startup-only':
                failure.update({'timedPageReads': 0, 'pageMatrixCoverage': 'none'})
            (output / failure_name).write_text(json.dumps(failure,indent=2)+'\n',encoding='utf-8')
            raise RuntimeError('isolated library page matrix failed; sanitized failure artifact retained') from None
        finally:
            if launcher.poll() is None:
                launcher.send_signal(signal.SIGINT)
                try:
                    launcher.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    launcher.terminate()
                    launcher.wait(timeout=5)
            # Verify the exact binary and isolated XDG profile before any fallback stop.
            try:
                pid = app_pid(binary, profile, timeout=0.1)
            except TimeoutError:
                pid = None
            if pid is not None:
                exe = Path(f'/proc/{pid}/exe')
                env_file = Path(f'/proc/{pid}/environ')
                try:
                    same_binary = exe.resolve() == binary.resolve()
                    entries = set(env_file.read_bytes().split(b'\0'))
                    same_profile = f'XDG_CONFIG_HOME={profile}/config'.encode() in entries and f'XDG_DATA_HOME={profile}/data'.encode() in entries
                except OSError:
                    same_binary = same_profile = False
                if same_binary and same_profile:
                    os.kill(pid, signal.SIGTERM)



def build_parser():
    p = argparse.ArgumentParser(description=__doc__, epilog='Creates fresh isolated profiles under /tmp; performs no playback or provider requests. Run only after the immutable app binary is frozen.')
    p.add_argument('--binary', required=True, type=Path)
    p.add_argument('--output', required=True, type=Path)
    p.add_argument('--counts', nargs='+', type=int, default=list(COUNTS))
    p.add_argument('--fixture-mode', choices=FIXTURE_MODES, default='catalog', help='catalog-only (default) or asset-backed synthetic tiny-WAV hardlink fixture')
    p.add_argument('--grant-layout', choices=GRANT_LAYOUTS, default='external', help='external (default, preserves prior matrix) or configured-root (asset-backed only; all fixture paths are beneath the isolated musicDirectory)')
    p.add_argument('--projection-state', choices=('initial', 'ready'), default='initial', help='initial (default) includes startup/backfill latency; ready waits for exact-cache projections before measuring')
    p.add_argument('--mode', choices=('matrix', 'startup-only'), default='matrix', help='matrix (default) runs MCP page reads; startup-only measures launch/registration and performs zero page reads')
    p.add_argument('--startup-timeout-seconds', type=bounded_startup_timeout, default=20, help='bounded app/MCP registration wait (default 20; maximum 180); use a larger value only for explicitly labeled slow-startup qualification')
    p.add_argument('--ticks-per-second', type=int, default=os.sysconf('SC_CLK_TCK'))
    return p


def main(argv=None):
    p = build_parser()
    a = p.parse_args(argv)
    if a.mode == 'startup-only' and a.projection_state != 'initial':
        p.error('--projection-state is not used with --mode startup-only')
    if a.grant_layout == 'configured-root' and a.fixture_mode != 'asset-backed':
        p.error('--grant-layout configured-root requires --fixture-mode asset-backed')
    if any(n not in COUNTS for n in a.counts):
        p.error('counts must be selected from 1000 5000 20000 50000')
    a.binary = a.binary.resolve(strict=True)
    a.output.mkdir(parents=True, exist_ok=True)
    results=[]
    index=a.output / 'matrix-index.json'
    projection_state = 'not_applicable' if a.mode == 'startup-only' else a.projection_state
    try:
        for count in a.counts:
            results.append(run_count(a.binary, a.output, count, a.ticks_per_second, a.fixture_mode, a.projection_state, a.startup_timeout_seconds, a.mode, a.grant_layout))
            index.write_text(json.dumps({'schemaVersion':1,'mode':a.mode,'fixtureMode':a.fixture_mode,'grantLayout':a.grant_layout,'projectionState':projection_state,'startupRegistrationTimeoutSeconds':a.startup_timeout_seconds,'results':results,'failureCount':0},indent=2)+'\n')
    except Exception:
        index.write_text(json.dumps({'schemaVersion':1,'mode':a.mode,'fixtureMode':a.fixture_mode,'grantLayout':a.grant_layout,'projectionState':projection_state,'startupRegistrationTimeoutSeconds':a.startup_timeout_seconds,'results':results,'failureCount':1,'failureCategory':'matrix_run_failed'},indent=2)+'\n')
        raise SystemExit('library page matrix failed; sanitized per-count artifact retained')
    print(json.dumps({'complete':True,'countRuns':len(results),'mode':a.mode,'fixtureMode':a.fixture_mode,'grantLayout':a.grant_layout,'projectionState':projection_state,'startupRegistrationTimeoutSeconds':a.startup_timeout_seconds,'failureCount':0,'output':'matrix-index.json'}))

if __name__ == '__main__':
    main()
