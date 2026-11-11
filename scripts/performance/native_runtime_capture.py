#!/usr/bin/env python3
"""Capture a packaged Linux player's output in an already isolated audio server.

Requires the reviewed private PipeWire controller and extracted Xvfb used by the
local qualification environment. Never connects to the normal audio server.
The payload can be an extracted DEB; that is runtime evidence, not an OS install.
"""
from pathlib import Path
from contextlib import closing
import argparse
import hashlib
import importlib.util
import json
import math
import os
import re
import signal
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile
import time
import uuid
import wave
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/performance'))
from run_isolated_memory import track_record
from native_schedstat_sampler import enrich_phase_intervals

STREAMING_PROVIDERS = {'youtube', 'soundcloud', 'monochrome'}
PRIVATE_UI_ACTOR_DIR = ROOT / 'scripts/performance/private_ui_actor'
PRIVATE_UI_ACTOR_HASHES = {
    'first_play_actor.py': 'd4fbbba021e7155d5233f1d92d54db831583535b2852eb209295507e8f587f66',
    'private_actor_base.py': '0e4b31647ced692c7832b5eec5c447784c739008258516e49e94b49ee37f1b7d',
    'radio_ui_actor.py': '9e32c3717df7be0c2a26e7c3b5c08f4542c2508bb5623a49d1bc05fd77a2d2d5',
    'private_app_launcher.py': '57b9f74642f5dec02dd708880f972faa2c3d275ddefcefe8d7964eabe7c6d8e0',
}
AT_SPI_SERVICE_TEXT = ('[D-BUS Service]\nName=org.a11y.Bus\n'
                       'Exec=/usr/lib/at-spi-bus-launcher --launch-immediately\n')
REPORTED_TRACK_FIELDS = {
    'id', 'provider', 'providerId', 'sourceUrl', 'artist', 'title', 'version', 'album', 'albumArtist',
    'recordingDate', 'trackNumber', 'trackTotal', 'discNumber', 'discTotal', 'remixers', 'playbackRange',
    'label', 'genre', 'normalizedGenre', 'durationSeconds', 'bpm', 'key', 'releaseDate', 'dateAdded',
    'artworkUrl', 'isrc', 'catalogNumber', 'rank', 'previewUrl', 'favorite', 'disposition', 'localPath',
    'localAvailability', 'assetId', 'provenance',
}

def file_sha256(path):
    digest=hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda:stream.read(1024*1024),b''):digest.update(block)
    return digest.hexdigest()

def bind_app_launch_monotonic_origin(environment, monotonic_ns):
    if not isinstance(monotonic_ns,int) or monotonic_ns<0:
        raise ValueError('Monotonic app launch origin must be a nonnegative integer')
    origin=str(monotonic_ns)
    environment['BURRATA_QUAL_APP_LAUNCH_MONOTONIC_NS']=origin
    return dict(environment)

def _proc_start_time_ticks(pid):
    raw=Path(f'/proc/{pid}/stat').read_text()
    return raw[raw.rfind(')')+2:].split()[19]

def validate_appimage_launch_clock_receipt(profile,appimage,expected_raw_sha256,app_pid,
                                           now_monotonic_ns=None,now_unix_ms=None):
    profile=Path(profile).resolve(strict=True);path=profile/'app-launch-clock.json'
    if path.is_symlink() or not path.is_file():raise ValueError('AppImage launch clock receipt is missing or unsafe')
    stat=path.stat()
    if stat.st_uid!=os.getuid() or stat.st_mode&0o777!=0o600:
        raise ValueError('AppImage launch clock receipt ownership or mode is invalid')
    receipt=json.loads(path.read_text(encoding='utf-8'))
    required={'schemaVersion','profile','appImagePath','appImageSha256','launcherPid','launcherStartTimeTicks',
        'appImagePid','appImageStartTimeTicks','appLaunchMonotonicNs','appLaunchUnixMs'}
    if not isinstance(receipt,dict) or set(receipt)!=required or receipt.get('schemaVersion')!=1:
        raise ValueError('AppImage launch clock receipt shape is invalid')
    image=Path(appimage).resolve(strict=True)
    if (receipt.get('profile')!=str(profile) or receipt.get('appImagePath')!=str(image)
            or receipt.get('appImageSha256')!=expected_raw_sha256):
        raise ValueError('AppImage launch clock receipt does not match the pinned image/profile')
    if (not isinstance(receipt.get('launcherPid'),int) or receipt['launcherPid']<=1
            or not isinstance(receipt.get('appImagePid'),int) or receipt['appImagePid']<=1):
        raise ValueError('AppImage launch clock receipt PID binding is invalid')
    for key in ('launcherStartTimeTicks','appImageStartTimeTicks'):
        if not isinstance(receipt.get(key),str) or not receipt[key].isdigit():
            raise ValueError('AppImage launch clock receipt process start time is invalid')
    try:
        launcher_ticks=_proc_start_time_ticks(receipt['launcherPid'])
        appimage_ticks=_proc_start_time_ticks(receipt['appImagePid'])
    except OSError as error:
        raise ValueError('AppImage launch clock receipt process disappeared') from error
    if launcher_ticks!=receipt['launcherStartTimeTicks']:
        raise ValueError('AppImage launch clock receipt launcher process changed')
    if appimage_ticks!=receipt['appImageStartTimeTicks']:
        raise ValueError('AppImage launch clock receipt raw image process changed')
    if app_pid not in _descendant_processes(receipt['launcherPid']):
        raise ValueError('AppImage launch clock receipt does not own the selected native process')
    now_monotonic_ns=time.monotonic_ns() if now_monotonic_ns is None else now_monotonic_ns
    now_unix_ms=time.time_ns()//1_000_000 if now_unix_ms is None else now_unix_ms
    launch_ns=receipt.get('appLaunchMonotonicNs');launch_ms=receipt.get('appLaunchUnixMs')
    if (not isinstance(launch_ns,int) or isinstance(launch_ns,bool) or launch_ns<0 or launch_ns>now_monotonic_ns
            or now_monotonic_ns-launch_ns>120_000_000_000):
        raise ValueError('AppImage launch monotonic timestamp is outside the permitted bound')
    if (not isinstance(launch_ms,int) or isinstance(launch_ms,bool) or launch_ms<1_600_000_000_000
            or launch_ms>now_unix_ms+10_000):
        raise ValueError('AppImage launch wall-clock timestamp is outside the permitted bound')
    return receipt

def verify_private_ui_actor(directory=PRIVATE_UI_ACTOR_DIR, expected_hashes=PRIVATE_UI_ACTOR_HASHES):
    directory=Path(directory)
    if directory.is_symlink() or directory.resolve(strict=True)!=directory:
        raise ValueError('Private GUI actor directory must be a real pinned path')
    actual={}
    for name,expected in expected_hashes.items():
        path=directory/name
        if path.is_symlink() or not path.is_file() or file_sha256(path)!=expected:
            raise ValueError(f'Private GUI actor asset failed its SHA-256 pin: {name}')
        actual[name]=expected
    return {'directory':str(directory),'entryPoint':str(directory/'first_play_actor.py'),
        'supportDirectory':str(directory),'assets':actual,
        'scope':'private AT-SPI named Play action; no coordinate or MCP playback action'}

def validate_appimage_pin(appimage,payload,raw_sha256,elf_sha256):
    appimage=Path(appimage)
    if not raw_sha256 or not elf_sha256:raise ValueError('AppImage requires explicit raw and ELF SHA-256 pins')
    if appimage.is_symlink():raise ValueError('AppImage must be a non-symlink executable file')
    appimage=appimage.resolve(strict=True);binary=Path(payload).resolve(strict=True)/'usr/bin/burrata'
    if not appimage.is_file() or not os.access(appimage,os.X_OK):raise ValueError('AppImage must be a regular executable file')
    if file_sha256(appimage)!=raw_sha256:raise ValueError('AppImage raw SHA-256 does not match the explicit pin')
    if file_sha256(binary)!=elf_sha256:raise ValueError('Payload burrata ELF SHA-256 does not match the explicit AppImage pin')
    return appimage

APP_RUN_FINAL_EXEC=b'exec "$APPDIR/usr/bin/burrata" "$@"'
def instrument_appimage_apprun(payload,destination):
    payload=Path(payload).resolve(strict=True);source=payload/'AppRun';resolved=source.resolve(strict=True)
    if not resolved.is_relative_to(payload) or not resolved.is_file() or not os.access(resolved,os.X_OK):
        raise ValueError('AppImage AppRun must resolve to an executable file inside the verified payload')
    original=resolved.read_bytes();lines=original.splitlines(keepends=True)
    matches=[index for index,line in enumerate(lines) if line.rstrip(b'\r\n')==APP_RUN_FINAL_EXEC]
    nonblank=[index for index,line in enumerate(lines) if line.strip()]
    if len(matches)!=1 or not nonblank or matches[0]!=nonblank[-1]:
        raise ValueError('AppImage AppRun must contain exactly one final Burrata exec line')
    index=matches[0];newline=b'\r\n' if lines[index].endswith(b'\r\n') else b'\n'
    if index and not lines[index-1].endswith((b'\n',b'\r')):lines[index-1]+=newline
    lines.insert(index,b'export BURRATA_IPC_COUNTER_PID=$$'+newline);instrumented=b''.join(lines)
    destination=Path(destination)
    if destination.exists() or destination.is_symlink():raise ValueError('Private instrumented AppRun destination must be new')
    with destination.open('xb') as stream:stream.write(instrumented)
    destination.chmod(0o700)
    return {'sourcePath':str(resolved),'sourceSha256':hashlib.sha256(original).hexdigest(),
        'instrumentedPath':str(destination.resolve(strict=True)),
        'instrumentedSha256':hashlib.sha256(instrumented).hexdigest(),
        'instrumentationScope':'one export immediately before the unique final exec of the verified mounted Burrata ELF; AppRun shell PID is preserved by exec'}


def _reject_sensitive_fixture_values(value, path='fixture'):
    forbidden_key = re.compile(r'(?:access.?token|refresh.?token|authorization|cookie|password|secret|credential|api.?key)', re.I)
    forbidden_text = re.compile(r'(?:\bbearer\s+[A-Za-z0-9._~-]{8,}|(?:access_token|refresh_token|token|api_key|apikey)=)', re.I)
    if isinstance(value, dict):
        for key, child in value.items():
            if forbidden_key.search(str(key)):
                raise ValueError(f'{path} contains a credential-like field')
            _reject_sensitive_fixture_values(child, f'{path}.{key}')
    elif isinstance(value, list):
        for index, child in enumerate(value):
            _reject_sensitive_fixture_values(child, f'{path}[{index}]')
    elif isinstance(value, str) and forbidden_text.search(value):
        raise ValueError(f'{path} contains credential-like text')


def load_reported_track_fixture(source, private_destination):
    """Validate and copy only sanitized remote catalog tracks into a disposable profile."""
    source = Path(source)
    if source.is_symlink():
        raise ValueError('Reported-track fixture must not be a symlink')
    source = source.resolve(strict=True)
    if not source.is_file():
        raise ValueError('Reported-track fixture must be a regular JSON file')
    document = json.loads(source.read_text(encoding='utf-8'))
    if not isinstance(document, dict) or set(document) != {'schemaVersion', 'tracks'} or document.get('schemaVersion') != 1:
        raise ValueError('Reported-track fixture must contain schemaVersion 1 and tracks only')
    tracks = document.get('tracks')
    if not isinstance(tracks, list) or not 1 <= len(tracks) <= 8:
        raise ValueError('Reported-track fixture needs between one and eight tracks')
    _reject_sensitive_fixture_values(document)
    sanitized = []
    identifiers = set()
    for index, original in enumerate(tracks):
        if not isinstance(original, dict) or set(original) - REPORTED_TRACK_FIELDS:
            raise ValueError(f'Reported track {index + 1} has unsupported fields')
        for field in ('id', 'provider', 'providerId', 'sourceUrl', 'artist', 'title'):
            value = original.get(field)
            if not isinstance(value, str) or not value.strip() or any(ord(char) < 32 for char in value):
                raise ValueError(f'Reported track {index + 1} is missing a valid {field}')
        for field in ('id','provider','providerId','artist','title'):
            text=original[field]
            if '://' in text.lower() or re.search(r'\bwww\.',text,re.I):
                raise ValueError(f'Reported track {index + 1} has an unsafe {field}')
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._:-]{0,255}',original['id']):
            raise ValueError(f'Reported track {index + 1} has an unsafe ID')
        if original.get('localPath') not in (None, '') or original.get('assetId') not in (None, ''):
            raise ValueError(f'Reported track {index + 1} must not reference a local asset')
        if original.get('localAvailability', 'none') != 'none':
            raise ValueError(f'Reported track {index + 1} must be remote-only')
        duration=original.get('durationSeconds')
        if duration is not None and (not isinstance(duration,(int,float)) or not math.isfinite(duration) or duration<=0):
            raise ValueError(f'Reported track {index + 1} has an invalid duration')
        if not isinstance(original.get('favorite',False),bool):
            raise ValueError(f'Reported track {index + 1} has an invalid favorite field')
        if original['id'] in identifiers:
            raise ValueError('Reported-track IDs must be unique')
        identifiers.add(original['id'])
        source_url = original['sourceUrl']
        parsed_url=urlsplit(source_url)
        if parsed_url.scheme not in {'http','https'} or not parsed_url.hostname or parsed_url.username or parsed_url.password:
            raise ValueError(f'Reported track {index + 1} sourceUrl must be public HTTP(S) without credentials')
        track = dict(original)
        track.update(favorite=track.get('favorite',False),localPath=None,localAvailability='none',
            assetId=None,previewUrl=None,artworkUrl=None)
        sanitized.append(track)
    destination = Path(private_destination)
    destination.write_text(json.dumps({'schemaVersion': 1, 'tracks': sanitized}, ensure_ascii=False) + '\n', encoding='utf-8')
    destination.chmod(0o600)
    digest = hashlib.sha256(destination.read_bytes()).hexdigest()
    return sanitized, digest


def streaming_fallback_policy_settings(enabled, sources):
    """Build an authoritative v1 fallback setting and its legacy mirror."""
    sources = list(sources)
    return {
        'streamingFallbackEnabled': enabled,
        'streamingFallbackSources': sources,
        'streamingFallbackPolicyVersion': 1,
        'bandcampFallbackEnabled': 'bandcamp' in sources,
    }


def parse_streaming_fallback_sources(value, preferred):
    if value is None or value == '':
        return []
    sources = [item.strip() for item in value.split(',')]
    if any(item not in STREAMING_PROVIDERS for item in sources) or len(set(sources)) != len(sources):
        raise ValueError('Fallback sources must be unique youtube, soundcloud, or monochrome names')
    if preferred in sources:
        raise ValueError('Fallback sources must not repeat the preferred streaming source')
    return sources


def reported_track_matching_policy(level='flexible', duration_policy='bounded',
                                   max_shorter_seconds=30, max_longer_seconds=30):
    """Return the reported-track matching settings and their truthful metadata."""
    return {
        'level': level,
        'allowOtherReleases': True,
        'allowOtherPerformances': True,
        'allowOtherMixes': True,
        'allowOtherArtists': False,
        'allowDifferentLengths': duration_policy == 'bounded',
        'maxShorterBySeconds': max_shorter_seconds,
        'maxLongerBySeconds': max_longer_seconds,
        'durationPolicy': duration_policy,
    }


def reported_track_matching_settings(policy):
    """Settings payload consumed by the app, derived from reported-track policy."""
    return policy['level'], dict(version=1,
        allowOtherReleases=policy['allowOtherReleases'],
        allowOtherPerformances=policy['allowOtherPerformances'],
        allowOtherMixes=policy['allowOtherMixes'],
        allowOtherArtists=policy['allowOtherArtists'],
        allowDifferentLengths=policy['allowDifferentLengths'],
        maxShorterBySeconds=policy['maxShorterBySeconds'],
        maxLongerBySeconds=policy['maxLongerBySeconds'])

def scratch_roots():
    """Keep disposable runtime data off a nearly full home filesystem when requested."""
    configured = os.environ.get('BURRATA_NATIVE_RUNTIME_SCRATCH_ROOT')
    root = Path(configured).resolve() if configured else ROOT / '.cache/native-runtime-tmp'
    if configured and not root.is_relative_to(Path('/tmp')):
        raise RuntimeError('Native runtime scratch override must remain under /tmp')
    root.mkdir(parents=True, exist_ok=True)
    capture_root = root.parent / 'native-runtime-captures'
    capture_root.mkdir(parents=True, exist_ok=True)
    return root, capture_root


def safe_startup_performance_trace(log_path):
    """Project only fixed startup/quality-worker timing lines from a private app log."""
    import re
    patterns = [
        (re.compile(r'^\[startup\] phase=([A-Za-z0-9_.-]{1,80}) elapsed_ms=(\d+)$'),
         lambda match: {'kind': 'startup_phase', 'phase': match.group(1), 'elapsedMs': int(match.group(2))}),
        (re.compile(r'^\[perf\] quality_facts\.select assets=(\d+) elapsed_ms=(\d+)$'),
         lambda match: {'kind': 'quality_facts_select', 'assets': int(match.group(1)), 'elapsedMs': int(match.group(2))}),
        (re.compile(r'^\[perf\] quality_facts\.hash item=(\d+) matches=(true|false) elapsed_ms=(\d+)$'),
         lambda match: {'kind': 'quality_facts_hash', 'item': int(match.group(1)), 'matches': match.group(2) == 'true', 'elapsedMs': int(match.group(3))}),
        (re.compile(r'^\[perf\] quality_facts\.inspect item=(\d+) saved=(true|false) elapsed_ms=(\d+)$'),
         lambda match: {'kind': 'quality_facts_inspect', 'item': int(match.group(1)), 'saved': match.group(2) == 'true', 'elapsedMs': int(match.group(3))}),
        (re.compile(r'^\[perf\] quality_facts\.inspect item=(\d+) outcome=(error) elapsed_ms=(\d+)$'),
         lambda match: {'kind': 'quality_facts_inspect', 'item': int(match.group(1)), 'outcome': match.group(2), 'elapsedMs': int(match.group(3))}),
        (re.compile(r'^\[perf\] quality_facts\.total elapsed_ms=(\d+)$'),
         lambda match: {'kind': 'quality_facts_total', 'elapsedMs': int(match.group(1))}),
    ]
    try:
        lines = Path(log_path).read_text(errors='replace').splitlines()
    except OSError:
        return []
    projected = []
    for line in lines:
        for pattern, project in patterns:
            match = pattern.fullmatch(line)
            if match:
                projected.append(project(match))
                break
    return projected


def incident_cases_passed(runtime):
    """Retain controller failures and require at least one completed case."""
    cases = runtime.get('cases')
    return bool(runtime.get('passed') is True and not runtime.get('error')
                and isinstance(cases, list) and cases
                and all(isinstance(case, dict) and case.get('passed') is True for case in cases))


def reported_track_case_gate(playback_progress_passed, captured_active_seconds, finite_pcm,
                             source_provider, expected_source_provider, startup_elapsed_ms,
                             startup_timeout_ms, pcm_phase_elapsed_ms, pcm_phase_window_ms):
    pcm_passed=bool(finite_pcm and isinstance(captured_active_seconds,(int,float))
                    and math.isfinite(captured_active_seconds) and captured_active_seconds>=4.5)
    provider_passed=(source_provider in STREAMING_PROVIDERS if expected_source_provider is None
                     else source_provider==expected_source_provider)
    startup_phase_passed=bool(isinstance(startup_elapsed_ms,(int,float)) and math.isfinite(startup_elapsed_ms)
        and 0<=startup_elapsed_ms<=startup_timeout_ms)
    pcm_phase_passed=bool(isinstance(pcm_phase_elapsed_ms,(int,float)) and math.isfinite(pcm_phase_elapsed_ms)
        and 5000<=pcm_phase_elapsed_ms<=pcm_phase_window_ms+250)
    return {'capturedPcmPassed':pcm_passed,'sourceProviderPassed':provider_passed,
            'startupPhasePassed':startup_phase_passed,'pcmPhasePassed':pcm_phase_passed,
            'passed':bool(playback_progress_passed and pcm_passed and provider_passed
                and startup_phase_passed and pcm_phase_passed)}


def run(args, **kwargs):
    return subprocess.run([str(x) for x in args], check=True, capture_output=True, **kwargs)


def host_baseline(audio):
    snapshot=audio.host_snapshot()
    query=subprocess.run(['pactl','-s',f'unix:/run/user/{os.getuid()}/pulse/native','list','short','sinks'],capture_output=True,text=True,timeout=3)
    names=[line.split('\t')[1] for line in query.stdout.splitlines() if '\t' in line]
    snapshot['testSinkPresent']=any(name.startswith(('burrata_native_','burrata_qual_')) for name in names)
    snapshot['inventoryReadable']=query.returncode==0
    return snapshot


def seed_private_atspi_activation(profile):
    """Create the single AT-SPI activation in a private startup session."""
    profile = Path(profile)
    if (profile.is_symlink() or not re.fullmatch(r'/tmp/burrata-launch\.[A-Za-z0-9_-]+', str(profile))
            or profile.resolve(strict=True) != profile or not profile.is_dir()
            or profile.stat().st_uid != os.getuid() or profile.stat().st_mode & 0o077
            or (profile / '.burrata-isolated-profile').read_text() != 'burrata isolated memory run\n'):
        raise RuntimeError('PRIVATE_ATSPI_PROFILE_INVALID')
    service_dir = profile / 'atspi-services'
    service = service_dir / 'org.a11y.Bus.service'
    runtime = profile / 'runtime'
    session_config = profile / 'atspi-session.conf'
    launcher = Path('/usr/lib/at-spi-bus-launcher')
    if any(path.exists() or path.is_symlink() for path in (service_dir, runtime, session_config)):
        raise RuntimeError('PRIVATE_ATSPI_FILES_ALREADY_EXIST')
    if not launcher.is_file() or launcher.is_symlink() or not os.access(launcher, os.X_OK):
        raise RuntimeError('PRIVATE_ATSPI_LAUNCHER_UNAVAILABLE')
    service_dir.mkdir(mode=0o700)
    runtime.mkdir(mode=0o700)
    service.write_text(AT_SPI_SERVICE_TEXT, encoding='utf-8')
    session_config.write_text(
        '<busconfig><type>session</type><keep_umask/><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>'
        f'<servicedir>{service_dir}</servicedir>'
        '<policy context="default"><allow send_destination="*" eavesdrop="true"/>'
        '<allow eavesdrop="true"/><allow own="*"/></policy></busconfig>',
        encoding='utf-8')
    service.chmod(0o600)
    session_config.chmod(0o600)
    digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    return {
        'serviceFile': service.name,
        'serviceFileSha256': digest(service),
        'serviceFileMode': '0600',
        'serviceDirectoryMode': '0700',
        'runtimeDirectory': str(runtime),
        'runtimeDirectoryMode': '0700',
        'launcherPath': str(launcher),
        'launcherSha256': digest(launcher),
        'sessionConfig': session_config.name,
        'sessionConfigSha256': digest(session_config),
        'sessionConfigMode': '0600',
        'scope': 'one startup attempt D-Bus session; only org.a11y.Bus activation added',
    }


def startup_atspi_launch_settings(profile, mode, audio_session_config, environment):
    """Bind AX setup only to the startup app and its matching private actor."""
    if mode != 'startup':
        return Path(audio_session_config), environment, None
    activation = seed_private_atspi_activation(profile)
    child_env = dict(environment)
    child_env.pop('NO_AT_BRIDGE', None)
    child_env['XDG_RUNTIME_DIR'] = str(Path(profile) / 'runtime')
    return Path(profile) / 'atspi-session.conf', child_env, activation


def seed(profile, resources, mode, args):
    spec = importlib.util.spec_from_file_location('installed_smoke', ROOT / 'scripts/release/smoke-installed.py')
    smoke = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(smoke)
    for folder in ['data/org.burrata.discovery', 'fixture', 'config', 'cache', 'state', 'tmp']:
        (profile / folder).mkdir(parents=True, exist_ok=True)
    (profile / '.burrata-isolated-profile').write_text('burrata isolated memory run\n')
    app_data = profile / 'data/org.burrata.discovery'
    reported_track_digest = None
    reported_tracks = []
    if args.reported_tracks:
        reported_tracks, reported_track_digest = load_reported_track_fixture(
            args.reported_tracks, profile / 'fixture/reported-tracks.json')
    aac = smoke.build_native_audio_fixture(resources, smoke.TARGETS['linux'], dict(os.environ), profile, app_data)
    records = []
    for name, left, right, hint in [('first', 440, 660, 4), ('second', 880, 1100, 24)]:
        path = profile / 'fixture' / f'{name}.wav'
        with wave.open(str(path), 'wb') as out:
            out.setnchannels(2);out.setsampwidth(2);out.setframerate(48000)
            # Natural-end/seek cases retain two seconds of real trailing silence.
            active = 12 if mode.startswith('crossfade') else 10
            seconds=60 if mode=='live-analysis' and name=='first' else 12
            noise_state=0x71f034
            for start in range(0, seconds * 48000, 4096):
                block = bytearray()
                for i in range(start, min(start + 4096, seconds * 48000)):
                    values = [int(6000 * math.sin(2 * math.pi * f * i / 48000)) if i < active * 48000 else 0 for f in (left, right)]
                    if mode=='live-analysis' and name=='first':
                        t=i/48000;phase=t%.5
                        chord=sum(math.sin(2*math.pi*f*t) for f in [261.63,329.63,392])*0.18
                        click=math.sin(2*math.pi*1200*phase)*math.exp(-phase*260)*.55 if phase<.012 else 0
                        sample=int(max(-.95,min(.95,chord+click))*32767)
                        values=[sample,sample]
                    if mode.startswith('stereo-') and name=='first':
                        noise_state=(noise_state*1664525+1013904223)&0xffffffff
                        pulse=i>=48000 and i<480000 and (i-48000)%96000<3840
                        noise=((noise_state>>8)/16777215*2-1)*.5 if pulse else 0
                        values=[int((.04*math.sin(2*math.pi*f*i/48000)+noise)*32767) if i<480000 else 0 for f in [left,right]]
                    block.extend(struct.pack('<hh', *values))
                out.writeframesraw(block)
        if mode.startswith('normalization-'):
            tagged=path.with_suffix('.flac')
            gain=-6 if name=='first' else -12
            run([resources/'tools/x86_64-unknown-linux-gnu/ffmpeg','-nostdin','-v','error','-i',path,
                 '-c:a','flac','-metadata',f'REPLAYGAIN_TRACK_GAIN={gain} dB',
                 '-metadata','REPLAYGAIN_TRACK_PEAK=0.2','-metadata','REPLAYGAIN_ALBUM_GAIN=-9 dB',
                 '-metadata','REPLAYGAIN_ALBUM_PEAK=0.2','-metadata','ALBUM=Qualification album',
                 '-metadata','ALBUMARTIST=Synthetic qualification',tagged])
            path=tagged
        if mode in ['stereo-aac','stereo-opus','stereo-seek-opus'] and name=='first':
            encoded=path.with_suffix('.m4a' if mode=='stereo-aac' else '.opus')
            codec=['-c:a','aac','-b:a','256k'] if mode=='stereo-aac' else ['-c:a','libopus','-b:a','160k']
            run([resources/'tools/x86_64-unknown-linux-gnu/ffmpeg','-nostdin','-v','error','-i',path,*codec,'-ar','48000',encoded]);path=encoded
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        track = track_record('local:' + digest, 'local', digest, 'Native runtime ' + name,
                             'Synthetic qualification', hint, str(path), str(uuid.uuid4()), path.suffix.lstrip('.'))
        if mode=='startup' and name=='first':track['durationSeconds']=12
        if mode=='live-analysis' and name=='first':track['durationSeconds']=60
        if mode.startswith('stereo-') and name=='first':track['durationSeconds']=12
        track['provenance']['manualFields']={'durationSeconds':True}
        if mode.startswith('normalization-'):track.update(album='Qualification album',albumArtist='Synthetic qualification')
        records.append(track)
    if mode == 'malformed-aac':
        # Double packet timing/duration fields without changing encoded samples.
        # Decoding must consume the actual samples rather than pad to this hint.
        fixture = next((profile / 'native-audio').glob('*.m4a')) if (profile / 'native-audio').exists() else None
        if fixture is None:
            with sqlite3.connect(app_data / 'burrata.sqlite') as db:
                fixture = Path(json.loads(db.execute('select metadata from catalog_listings where id=?', (aac['trackId'],)).fetchone()[0])['localPath'])
        data = bytearray(fixture.read_bytes())
        changed = []
        def boxes(start, end):
            offset = start
            while offset + 8 <= end:
                size = int.from_bytes(data[offset:offset+4], 'big')
                kind = bytes(data[offset+4:offset+8])
                if size < 8 or offset + size > end: break
                if kind in [b'moov', b'trak', b'mdia', b'edts', b'minf', b'stbl']:
                    boxes(offset + 8, offset + size)
                elif kind==b'stts':
                    entries=int.from_bytes(data[offset+12:offset+16],'big')
                    for n in range(entries):
                        at=offset+20+n*8
                        before=int.from_bytes(data[at:at+4],'big')
                        data[at:at+4]=(before*2).to_bytes(4,'big')
                        changed.append({'box':'stts','before':before,'after':before*2})
                elif kind in [b'mvhd', b'mdhd', b'tkhd', b'elst']:
                    version = data[offset+8]
                    at = offset + ((36 if version == 1 else 28) if kind==b'tkhd' else 16 if kind==b'elst' else (32 if version == 1 else 24))
                    width = 8 if version == 1 else 4
                    before = int.from_bytes(data[at:at+width], 'big')
                    data[at:at+width] = (before*2).to_bytes(width, 'big')
                    changed.append({'box':kind.decode(), 'before':before, 'after':before*2})
                offset += size
        boxes(0, len(data))
        if len(changed) < 2: raise RuntimeError('AAC duration fixture was not patched')
        fixture.write_bytes(data)
        (profile / 'malformed-duration.json').write_text(json.dumps(changed, indent=2))
        # Register the modified bytes with their real hash; only timing is bad.
        (app_data / 'burrata.sqlite').unlink()
        aac=smoke.register_native_audio_fixture(app_data,fixture,12,profile)
    incidents = []
    if args.reported_tracks:
        incidents = reported_tracks
    elif mode in ['incidents','broken-download','recovery-baseline','broken-playable','decoder-recovery','full-incidents']:
        sources = [Path('/tmp') / f'burrata-{name}-saved-track.json' for name in ['tainy','manana','veinte','amor']]
        sources += [Path('/tmp') / f'burrata-django-candidate-{n}.json' for n in [4,5,6]]
        incidents = [json.loads(path.read_text()) for path in sources]
        if mode=='full-incidents':incidents=[incidents[4],incidents[6],incidents[5]]
        if mode=='decoder-recovery':
            incidents=incidents[:1]
            controlled=profile/'fixture/controlled-decoder.opus'
            ffmpeg=resources/'tools/x86_64-unknown-linux-gnu/ffmpeg'
            run([ffmpeg,'-nostdin','-v','error','-stream_loop','-1','-i',records[0]['localPath'],'-t',str(incidents[0]['durationSeconds']),'-c:a','libopus','-b:a','32k','-vbr','off',controlled])
            scratch=ROOT/'.cache/native-runtime-tmp'/profile.name;scratch.mkdir(parents=True,exist_ok=True)
            wrapper=scratch/'ffmpeg-fail-once';flag=scratch/'decoder-fault-injected'
            wrapper.write_text('#!/usr/bin/python3\nimport os,sys\nfrom pathlib import Path\nflag=Path('+repr(str(flag))+')\nif any(arg.endswith("controlled-decoder.opus") for arg in sys.argv) and "f32le" in sys.argv:\n try:\n  fd=os.open(flag,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600);os.close(fd);sys.stderr.write("Controlled decoder failure before PCM\\n");sys.exit(23)\n except FileExistsError:pass\nos.execv('+repr(str(ffmpeg))+',['+repr(str(ffmpeg))+',*sys.argv[1:]])\n')
            wrapper.chmod(0o700)
            incidents[0].update(localPath=str(controlled),localAvailability='available',assetId=str(uuid.uuid4()))
        if mode in ['recovery-baseline','broken-playable']:
            incidents=incidents[5:6]
        if mode in ['broken-download','broken-playable']:
            incidents=incidents[:1]
            broken=profile/'fixture/broken-download.mp3';broken.write_bytes(b'not an audio stream: controlled fixture')
            incidents[0].update(localPath=str(broken),localAvailability='available',assetId=str(uuid.uuid4()))
    if mode in ['nipsey','full-nipsey','source-audition']:incidents=json.loads((ROOT/'scripts/performance/fixtures/nipsey-saved-tracks.json').read_text())
    if mode=='source-audition':
        incidents=incidents[:1]
        controlled=profile/'fixture/audition-local.wav'
        run([resources/'tools/x86_64-unknown-linux-gnu/ffmpeg','-nostdin','-v','error','-i',records[0]['localPath'],'-c:a','pcm_s16le','-metadata','comment=source audition control',controlled])
        incidents[0].update(localPath=str(controlled),localAvailability='available',assetId=str(uuid.uuid4()))
    db = sqlite3.connect(app_data / 'burrata.sqlite')
    with db:
        for track in records + incidents:
            tid = track['id']
            db.execute('insert into works(id,title,artist) values(?,?,?)', ('work:'+tid, track['title'], track['artist']))
            db.execute('insert into recordings(id,work_id,version) values(?,?,?)', (tid, 'work:'+tid, track.get('version')))
            db.execute('insert into catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) values(?,?,?,?,?,?,?)',
                       (tid, track['provider'], track['providerId'], tid, track.get('sourceUrl',''), json.dumps(track), '2026-10-31T00:00:00Z'))
            if track.get('localPath'):
                path = Path(track['localPath'])
                asset = {'id':track['assetId'],'recordingId':tid,'path':str(path),'hash':hashlib.sha256(path.read_bytes()).hexdigest(),
                    'sourceCodec':path.suffix.lstrip('.'),'finalCodec':path.suffix.lstrip('.'),'sampleRate':48000,'channels':2,'bitDepth':16 if path.suffix=='.wav' else None,
                    'durationSeconds':track['durationSeconds'],'sizeBytes':path.stat().st_size,
                    'processing':'existing-local-file','track':track}
                db.execute('insert into local_assets(id,recording_id,path,hash,payload) values(?,?,?,?,?)',
                           (track['assetId'],tid,str(path),asset['hash'],json.dumps(asset)))
        reported_options=bool(args.reported_tracks)
        if reported_options:
            preferred_source=args.preferred_streaming_source or 'youtube'
            fallback_sources=list(args.fallback_sources or [])
            if args.disable_streaming_fallback and fallback_sources:
                raise ValueError('--disable-streaming-fallback cannot be combined with --fallback-sources')
            fallback_enabled=bool(fallback_sources) and not args.disable_streaming_fallback
        else:
            preferred_source='bandcamp' if mode in ['recovery-baseline','broken-playable'] else 'monochrome'
            fallback_sources=['monochrome','youtube','soundcloud'] if mode in ['recovery-baseline','broken-playable'] else ['youtube','soundcloud','bandcamp']
            fallback_enabled=True
        matching_policy = reported_track_matching_policy(
            getattr(args, 'reported_track_matching_level', None) or 'flexible',
            getattr(args, 'reported_track_duration_policy', None) or 'bounded',
            getattr(args, 'reported_track_max_shorter_seconds', None)
            if getattr(args, 'reported_track_max_shorter_seconds', None) is not None else 30,
            getattr(args, 'reported_track_max_longer_seconds', None)
            if getattr(args, 'reported_track_max_longer_seconds', None) is not None else 30)
        settings = dict(musicDirectory=str(profile/'fixture'),autoPreview=False,previewDelayMs=250,theme='dark',
            ytDlpPath='yt-dlp',ffmpegPath='ffmpeg',ffprobePath='ffprobe',jsRuntimePath='node',playlistDirectories=[],
            preferredStreamingSource=preferred_source,
            **streaming_fallback_policy_settings(fallback_enabled, fallback_sources),
            playbackDiagnosticsEnabled=True,playbackStartupTimeoutSeconds=args.reported_track_startup_timeout_seconds if reported_options else 20,
            liveBpmEnabled=mode=='live-analysis',
            crossfadeEnabled=mode.startswith('crossfade'),crossfadeSeconds=2,
            matchingLevel=matching_policy['level'] if reported_options else 'balanced' if mode in ['nipsey','full-nipsey'] else 'flexible',flexibleMatching=dict(version=1,
                allowOtherReleases=matching_policy['allowOtherReleases'] if reported_options else True,
                allowOtherPerformances=matching_policy['allowOtherPerformances'] if reported_options else False,
                allowOtherMixes=matching_policy['allowOtherMixes'] if reported_options else False,
                allowOtherArtists=False,
                allowDifferentLengths=matching_policy['allowDifferentLengths'] if reported_options else False,
                maxShorterBySeconds=matching_policy['maxShorterBySeconds'] if reported_options else 30,
                maxLongerBySeconds=matching_policy['maxLongerBySeconds'] if reported_options else 30))
        if mode=='decoder-recovery':settings['ffmpegPath']=str(wrapper)
        if mode=='source-audition':settings.update(preferredDownloadSource='youtube',downloadFallbackEnabled=False,downloadFallbackSources=[])
        for key, value in [('application',settings),('playbackEngine.v1','native'),('releaseSetupComplete.v1',True),('radioRecognitionEnabled.v1',False)]:
            db.execute('insert or replace into settings(key,value) values(?,?)',(key,json.dumps(value)))
        if mode.startswith('normalization-'):
            db.execute('insert or replace into settings(key,value) values(?,?)',('loudness.v1',json.dumps(dict(mode=mode.split('-')[1],targetLufs=-18,ceilingDbtp=-1,maximumBoostDb=6,albumTrackFallback=False,analyzeWhenIdle=False))))
    db.close()
    if mode in ['waveform','live-analysis']:
        storage=app_data/'localstorage/tauri_localhost_0.localstorage';storage.parent.mkdir()
        with sqlite3.connect(storage) as browser:
            browser.execute('CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB NOT NULL ON CONFLICT FAIL)')
            browser.execute('INSERT INTO ItemTable(key,value) VALUES(?,?)',('burrata.waveform-enabled.v1','true'.encode('utf-16-le')))
    if mode=='migration':
        queue={'tracks':[records[0],records[0]],'entryIds':['legacy-first','legacy-second'],'currentIndex':1,'repeat':'all'}
        storage=app_data/'localstorage/tauri_localhost_0.localstorage';storage.parent.mkdir()
        with sqlite3.connect(storage) as browser:
            browser.execute('CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB NOT NULL ON CONFLICT FAIL)')
            for key,value in [('burrata.playback-queue.v1',queue),('burrata.playback-position.v1',{'trackId':records[0]['id'],'entryId':'legacy-second','position':5,'savedAt':int(time.time()*1000)}),('burrata.playback-controls.v1',{'volume':.37,'shuffle':True})]:
                browser.execute('INSERT INTO ItemTable(key,value) VALUES(?,?)',(key,json.dumps(value).encode('utf-16-le')))
        stale_queue={'tracks':[records[1]],'entryIds':['stale-native'],'queueRevision':7}
        stale_session={'index':0,'currentEntryId':'stale-native','queueRevision':7,'position':9,'volume':.88,'repeat':'off','shuffle':False,'finished':False}
        with sqlite3.connect(app_data/'burrata.sqlite') as db:
            for key,value in [('playbackEngine.v1','webview'),('nativePlaybackQueue.v1',stale_queue),('nativePlaybackSession.v1',stale_session)]:
                db.execute('INSERT OR REPLACE INTO settings(key,value) VALUES(?,?)',(key,json.dumps(value)))
    ids = [track['id'] for track in records]
    if mode == 'duplicates': ids = [ids[0],ids[0]]
    runtime_cases=dict(ids=ids,aacId=aac['trackId'],incidents=incidents)
    if mode=='startup':
        first=records[0]
        runtime_cases['startupUiAction']={'mode':'track','title':'Native runtime first',
            'scope':'Recordings','occurrence':1,'expectedRows':1,'trackId':first['id'],'artist':first['artist']}
    if reported_options:
        runtime_cases['reportedTrackOptions']={
            'fixtureSha256':reported_track_digest,
            'expectedSourceProvider':args.expected_source_provider,
            'preferredStreamingSource':preferred_source,
            'streamingFallbackSources':fallback_sources,
            'streamingFallbackEnabled':fallback_enabled,
            'reportedTrackDiagnostics':args.reported_track_diagnostics,
            'startupTimeoutSeconds':args.reported_track_startup_timeout_seconds,
            'pcmPhaseWindowSeconds':args.reported_track_pcm_window_seconds,
            'matchingPolicy':matching_policy,
        }
    (profile/'fixture/runtime-cases.json').write_text(json.dumps(runtime_cases))
    return settings

def seed_pending_quality_assets(profile, count):
    """Add hash-valid hard-linked assets whose quality-facts hash is stale."""
    if count == 0:
        return 0
    app_data = profile / 'data/org.burrata.discovery'
    pending_root = profile / 'fixture/quality-pending'
    pending_root.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(app_data / 'burrata.sqlite') as db:
        row = db.execute('SELECT payload FROM local_assets ORDER BY rowid LIMIT 1').fetchone()
        if row is None:
            raise RuntimeError('Startup fixture needs a registered local asset')
        template = json.loads(row[0])
        source = Path(template['path'])
        if not source.is_file():
            raise RuntimeError('Startup fixture source file is missing')
        for index in range(count):
            asset = json.loads(json.dumps(template))
            asset_id = f'quality-pending-{index + 1:03d}'
            path = pending_root / f'{asset_id}{source.suffix}'
            os.link(source, path)
            asset.update(id=asset_id, path=str(path))
            asset['track']['localPath'] = str(path)
            asset['track'].setdefault('provenance', {})['localAudioFactsHash'] = 'pending-fixture'
            db.execute(
                'INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES(?,?,?,?,?)',
                (asset_id, asset['recordingId'], str(path), asset['hash'], json.dumps(asset)),
            )
    return count


def summarize_capture(path, mode, start_seconds=0):
    import numpy as np
    pcm = np.fromfile(path,dtype='<f4',offset=max(0,int(start_seconds*48000))*8);pcm = pcm[:len(pcm)//2*2].reshape(-1,2)
    window = 12000;bins=[]
    for start in range(0,len(pcm)-window+1,window):
        block=pcm[start:start+window];t=np.arange(window)/48000;w=np.hanning(window)
        def tone(channel,f):return float(abs(np.dot(block[:,channel]*w,np.exp(-2j*np.pi*f*t)))*2/w.sum())
        bins.append(dict(second=start/48000,rms=float(np.sqrt(np.mean(block**2))),firstLeft=tone(0,440),firstRight=tone(1,660),
            secondLeft=tone(0,880),secondRight=tone(1,1100),
            wrongLeft=max(tone(0,660),tone(0,1100)),wrongRight=max(tone(1,440),tone(1,880))))
    first=[b for b in bins if min(b['firstLeft'],b['firstRight'])>.02]
    second=[b for b in bins if min(b['secondLeft'],b['secondRight'])>.02]
    overlap=[b for b in bins if min(b['firstLeft'],b['firstRight'],b['secondLeft'],b['secondRight'])>.01]
    stereo=all(max(b['wrongLeft'],b['wrongRight'])<.01 for b in bins)
    result=dict(frames=len(pcm),durationSeconds=len(pcm)/48000,peak=float(np.max(np.abs(pcm))) if len(pcm) else 0,
        stereoSeparated=stereo,firstToneSeconds=len(first)*.25,secondToneSeconds=len(second)*.25,
        firstToneStart=first[0]['second'] if first else None,secondToneStart=second[0]['second'] if second else None,
        overlapSeconds=len(overlap)*.25,bins=bins)
    def quiet_interval(start, end):
        # Ignore quarter-second edge bins, but require a real quiet interval.
        quiet = [b for b in bins if b['second'] >= start-1e-9 and b['second']+.25 <= end+1e-9]
        return len(quiet) >= 3 and all(b['rms'] < .003 for b in quiet)
    result['passed']=stereo and result['peak']<.99 and bool(first)
    if mode in ['natural','duplicates']:
        result['passed'] &= result['firstToneSeconds'] >= (19 if mode=='duplicates' else 9.5)
        if mode=='natural':result['passed'] &= result['secondToneSeconds']>=9.5 and 11.5<=result['secondToneStart']-result['firstToneStart']<=12.5 and 11.5<=result['durationSeconds']-result['secondToneStart']<=14.2
        else:
            starts=[b['second'] for i,b in enumerate(first) if i==0 or b['second']-first[i-1]['second']>.5]
            result['passed'] &= len(starts)==2 and 11.5<=starts[1]-starts[0]<=12.5 and 11.5<=result['durationSeconds']-starts[-1]<=14.2
        if first:
            # Each seeded recording has ten seconds of tone plus two seconds of
            # actual silence. Duration alone cannot prove those samples survived.
            anchors = [result['firstToneStart'], result['secondToneStart']] if mode=='natural' else starts
            result['realTrailingSilence'] = bool(anchors) and all(anchor is not None and quiet_interval(anchor+10.5, anchor+11.5) for anchor in anchors)
            result['passed'] &= result['realTrailingSilence'] and result['overlapSeconds']<=.25
    elif mode=='seek':
        # Seek targets10.5s, inside the real silent tail. Bound the pre-seek
        # tone and require captured silence, not just an advancing native clock.
        quiet_tail = result['durationSeconds']-(first[-1]['second']+.25) if first else 0
        result['seekSilentTailSeconds'] = quiet_tail
        result['seekTailQuiet'] = bool(first) and quiet_interval(first[-1]['second']+.5, result['durationSeconds'])
        result['passed'] &= .75<=result['firstToneSeconds']<=2 and not second and 1.25<=quiet_tail<=3.25 and result['seekTailQuiet']
    elif mode.startswith('crossfade'):
        onset = result['secondToneStart']-result['firstToneStart'] if first and second else None
        final_span = result['durationSeconds']-result['secondToneStart'] if second else 0
        result['crossfadeOnsetSeconds'] = onset
        result['crossfadeFinalSpanSeconds'] = final_span
        result['passed'] &= 11<=result['firstToneSeconds']<=12.75 and 11<=result['secondToneSeconds']<=12.75 and 1.25<=result['overlapSeconds']<=2.5 and onset is not None and 9.5<=onset<=11 and 11.5<=final_span<=14.2
    elif mode in ['aac','malformed-aac']:
        result['passed'] &= 11<=result['firstToneSeconds']<=12.75 and not second and 11.5<=result['durationSeconds']-result['firstToneStart']<=14.2
    elif mode=='startup':
        result['startupPcmGate']=startup_pcm_gate(result)
        result['passed'] &= result['startupPcmGate']['passed']
    return result


def startup_pcm_gate(result):
    """Require real stereo fixture tone and bounded post-gain monitor RMS."""
    import numpy as np
    bins=result.get('bins') if isinstance(result,dict) else None
    bins=bins if isinstance(bins,list) else []
    first=[b for b in bins if isinstance(b,dict)
           and min(b.get('firstLeft',0),b.get('firstRight',0))>.02]
    active_rms=[b['rms'] for b in first if isinstance(b.get('rms'),(int,float))
                and math.isfinite(b['rms'])]
    median_rms=float(np.median(active_rms)) if active_rms else None
    checks={
        'firstToneAtLeast2_5Seconds': isinstance(result.get('firstToneSeconds'),(int,float))
            and result['firstToneSeconds']>=2.5,
        'stereoSeparated': result.get('stereoSeparated') is True,
        'notClipping': isinstance(result.get('peak'),(int,float))
            and math.isfinite(result['peak']) and result['peak']<.99,
        'postGainMonitorRmsInRange': median_rms is not None and .02<=median_rms<=.35,
    }
    return {'passed':all(checks.values()),'checks':checks,'firstToneBins':len(first),
            'firstToneActiveRmsMedian':median_rms,'rmsRange':[.02,.35],
            'captureStage':'private Pulse monitor after Player gain'}



def assess_normalization(bins, mode):
    import numpy as np
    gains={'off':[0,0],'track':[-6,-12],'album':[-9,-9]}[mode]
    measured=[]
    for left,right in [('firstLeft','firstRight'),('secondLeft','secondRight')]:
        active=[b for b in bins if min(b[left],b[right])>.025]
        measured.append([float(np.median([b[key] for b in active])) if active else 0 for key in [left,right]])
    expected=[6000/32768*10**(gain/20) for gain in gains]
    passed=all(abs(actual-target)<target*.05 for channels,target in zip(measured,expected) for actual in channels)
    return dict(passed=passed,expectedGainDb=gains,expectedChannelAmplitudes=expected,measuredChannelAmplitudes=measured)



def assess_audition_queue_checkpoint(profile):
    """Read only the harness-owned disposable queue; API paths are redacted."""
    profile = Path(profile).resolve(strict=True)
    if (profile.parent != Path('/tmp') or not profile.name.startswith('burrata-launch.')
            or (profile / '.burrata-isolated-profile').read_text() != 'burrata isolated memory run\n'):
        raise ValueError('Disposable audition profile required')
    fixture = json.loads((profile / 'fixture/runtime-cases.json').read_text())['incidents'][0]
    expected = fixture.get('localPath')
    if not expected or not Path(expected).resolve().is_relative_to(profile / 'fixture'):
        return {'passed': False, 'reason': 'Missing owned local audition control path'}
    database = profile / 'data/org.burrata.discovery/burrata.sqlite'
    with closing(sqlite3.connect(database.resolve(strict=True).as_uri() + '?mode=ro', uri=True)) as db:
        row = db.execute("SELECT value FROM settings WHERE key='nativePlaybackQueue.v1'").fetchone()
    queue = json.loads(row[0]) if row else {}
    tracks = queue.get('tracks') or []
    retained = tracks[0] if tracks else {}
    return {'passed': bool(fixture.get('id') and retained.get('id') == fixture['id']
                           and retained.get('localPath') == expected),
            'queuePresent': bool(tracks), 'expectedLocalPathPresent': True}


def assess_source_audition_pcm(local, audition):
    import numpy as np
    def score(pcm):
        window=12000;t=np.arange(window)/48000;w=np.hanning(window);scores=[]
        for i in range(0,len(pcm)-window+1,window):
            segment=pcm[i:i+window]
            tone=lambda c,f:float(abs(np.dot(segment[:,c]*w,np.exp(-2j*np.pi*f*t)))*2/w.sum())
            scores.append(max(min(tone(0,440),tone(1,660)),min(tone(0,660),tone(1,440))))
        return float(np.median(scores)) if scores else 0
    local_score,online_score=score(local),score(audition)
    active=sum(float(np.sqrt(np.mean(audition[i:i+4800]**2)))>.0005 for i in range(0,len(audition)-4800,4800))*.1
    return dict(activeAuditionSeconds=active,localControlToneMedianAmplitude=local_score,
        auditionControlToneMedianAmplitude=online_score,passed=local_score>.1 and active>=9 and online_score<.03)



def assess_full_recording_pcm(pcm, cases, capture_started_at_ms):
    import numpy as np
    assessed=[]
    for index,original in enumerate(cases):
        item=dict(original);first_ms=item.get('startedAtUnixMs');last_ms=item.get('endedAtUnixMs')
        if last_ms is None and index+1<len(cases):
            last_ms=cases[index+1].get('startedAtUnixMs')
            item['endBoundarySource']='next occurrence first-playing timestamp'
        else:item['endBoundarySource']='recorded occurrence boundary'
        duration=item.get('decodedDuration',0);maximum=item.get('maximumPosition',0)
        bounds=all(isinstance(value,(int,float)) and math.isfinite(value) for value in [first_ms,last_ms,duration,maximum])
        active=0;finite=True
        if bounds and last_ms>first_ms:
            item['endedAtUnixMs']=last_ms
            first=max(0,int((first_ms-capture_started_at_ms)/1000*48000));last=min(len(pcm),int((last_ms-capture_started_at_ms)/1000*48000))
            for at in range(first,last-4800,4800):
                block=pcm[at:at+4800];finite &= bool(np.isfinite(block).all())
                active += .1 if float(np.sqrt(np.mean(block**2)))>.0005 else 0
        item['capturedActiveSeconds']=round(active,3)
        item['nearDecodedEnd']=bool(bounds and duration>30 and maximum>=duration-.75)
        item['passed']=bool(item['nearDecodedEnd'] and finite and active>=duration*.75)
        assessed.append(item)
    return assessed



def assess_stereo_markers(pcm, expected_markers):
    import numpy as np
    rate=48000
    if len(pcm)<rate:return dict(passed=False,markers=[],reason='capture too short')
    energy=np.convolve(np.mean(pcm**2,axis=1),np.ones(240)/240,mode='same')
    active=np.flatnonzero(energy>.01);groups=[]
    if len(active):
        starts=np.r_[0,np.flatnonzero(np.diff(active)>480)+1];ends=np.r_[starts[1:]-1,len(active)-1]
        groups=[(int(active[a]),int(active[b])+1) for a,b in zip(starts,ends) if 1200<=active[b]-active[a]<=9600]
    markers=[]
    for first,last in groups:
        block=pcm[max(0,first-480):min(len(pcm),last+480)].astype(np.float64)
        a=block[:,0]-block[:,0].mean();b=block[:,1]-block[:,1].mean()
        size=1<<(2*len(a)-1).bit_length();corr=np.fft.irfft(np.fft.rfft(a,size)*np.conj(np.fft.rfft(b,size)),size)
        lags=np.arange(-480,481);values=corr[lags%size];index=int(np.argmax(values))
        coherence=float(values[index]/max(1e-20,np.sqrt(np.dot(a,a)*np.dot(b,b))))
        markers.append(dict(second=first/rate,channelLagSamples=int(lags[index]),coherence=coherence))
    spacings=[b['second']-a['second'] for a,b in zip(markers,markers[1:])]
    timing=all(abs(value-2)<.03 for value in (spacings[1:] if expected_markers==3 else spacings))
    passed=len(markers)==expected_markers and timing and all(abs(m['channelLagSamples'])<=2 and m['coherence']>.85 for m in markers)
    return dict(passed=passed,markers=markers,spacingSeconds=spacings,toleranceSamples=2)


def _proc_executable(pid):
    try:
        return Path(f'/proc/{pid}/exe').resolve(strict=True)
    except OSError:
        return None


def _proc_children(pid):
    try:
        return [int(value) for value in Path(f'/proc/{pid}/task/{pid}/children').read_text().split()]
    except (OSError, ValueError):
        return []


def private_app_process_matches(executable, environment, binary_sha256, raw_appimage=None, profile=None,
                                expected_executable=None):
    """Match the one private app independently of its fresh AppImage mount path."""
    executable=Path(executable)
    try:
        if not executable.is_file() or file_sha256(executable)!=binary_sha256:return False
        if expected_executable is not None and executable.resolve(strict=True)!=Path(expected_executable).resolve(strict=True):return False
        if profile is not None and environment.get('XDG_DATA_HOME')!=str(Path(profile)/'data'):return False
        if raw_appimage is not None and environment.get('APPIMAGE')!=str(Path(raw_appimage).resolve(strict=True)):return False
    except OSError:
        return False
    return True


def select_unique_private_app(candidates, binary_sha256, raw_appimage=None, profile=None,expected_executable=None):
    matches=[(pid,Path(executable)) for pid,executable,environment in candidates
        if private_app_process_matches(executable,environment,binary_sha256,raw_appimage,profile,expected_executable)]
    if len(matches)!=1:raise RuntimeError('Could not uniquely identify private Burrata PID and executable')
    return matches[0]


def _descendant_processes(root_pid):
    pending=[root_pid];descendants=[];seen=set()
    while pending:
        parent=pending.pop()
        if parent in seen:continue
        seen.add(parent);children=_proc_children(parent);descendants.extend(children);pending.extend(children)
    return descendants


def _private_app_from_tree(root_pid,binary,binary_sha256,raw_appimage=None,profile=None):
    candidates=[]
    for pid in _descendant_processes(root_pid):
        executable=_proc_executable(pid)
        if executable is None:continue
        try:
            entries=Path(f'/proc/{pid}/environ').read_bytes().split(b'\0')
            environment={key.decode():value.decode(errors='replace') for entry in entries if b'=' in entry
                for key,value in [entry.split(b'=',1)]}
        except OSError:continue
        candidates.append((pid,executable,environment))
    return select_unique_private_app(candidates,binary_sha256,raw_appimage,profile,
        None if raw_appimage is not None else binary)


def _schedstat_app_targets(root_pid, binary, raw_appimage=None, profile=None):
    app_pid,app_executable=_private_app_from_tree(root_pid,binary,file_sha256(binary),raw_appimage,profile)
    return app_pid,[{'role':'burrata','pid':app_pid}]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--payload',type=Path,required=True)
    parser.add_argument('--appimage',type=Path,
                        help='Launch this exact AppImage while using --payload as its verified read-only FUSE resource root')
    parser.add_argument('--appimage-sha256',help='Required raw AppImage SHA-256 when --appimage is used')
    parser.add_argument('--appimage-elf-sha256',help='Required packaged burrata ELF SHA-256 when --appimage is used')
    parser.add_argument('--evidence',type=Path,required=True)
    parser.add_argument('--mode',choices=['startup','idle-ipc','natural','duplicates','seek','crossfade','crossfade-pause','aac','malformed-aac','incidents','broken-download','recovery-baseline','broken-playable','decoder-recovery','radio','performance','migration','mpris','full-incidents','waveform','normalization-off','normalization-track','normalization-album','live-analysis','nipsey','full-nipsey','source-audition','stereo-wav','stereo-aac','stereo-opus','stereo-seek-opus'],required=True)
    parser.add_argument('--build-profile',choices=['release','diagnostic'],default='release')
    parser.add_argument('--pending-quality-assets', type=int, choices=[0, 100], default=0,
                        help='Seed 0 or 100 valid-hash assets missing quality facts')
    parser.add_argument('--reported-tracks', type=Path,
                        help='Optional sanitized schemaVersion=1 reported-track fixture; valid only with incidents mode')
    parser.add_argument('--reported-track-diagnostics', action='store_true',
                        help='Opt in to a sanitized private Library-row lookup and playback phase/provider trace for reported-track incidents')
    parser.add_argument('--preferred-streaming-source', choices=sorted(STREAMING_PROVIDERS),
                        help='Preferred provider for reported-track incidents (default: youtube)')
    parser.add_argument('--fallback-sources',
                        help='Comma-separated fallback provider list for reported-track incidents; empty disables fallback')
    parser.add_argument('--disable-streaming-fallback', action='store_true',
                        help='Disable provider fallback for reported-track incidents')
    parser.add_argument('--expected-source-provider', choices=sorted(STREAMING_PROVIDERS),
                        help='Require every reported-track case to use this actual provider')
    parser.add_argument('--reported-track-startup-timeout-seconds', type=int, choices=range(5, 21), default=20,
                        help='Reported-track first-audio startup cap (maximum 20 seconds)')
    parser.add_argument('--reported-track-pcm-window-seconds', type=int, choices=range(5, 7), default=6,
                        help='Maximum separate post-first-audio capture phase for reported tracks (5 or 6 seconds)')
    parser.add_argument('--reported-track-matching-level', choices=['exact', 'balanced', 'flexible'],
                        help='Recording match level for reported-track incidents (default: flexible)')
    parser.add_argument('--reported-track-duration-policy', choices=['bounded', 'existing'],
                        help='Duration matching for reported tracks: bounded allows a duration difference within the configured caps; existing keeps the normal duration rule (default: bounded)')
    parser.add_argument('--reported-track-max-shorter-seconds', type=int, choices=range(0, 3601),
                        help='Maximum seconds the matched recording may be shorter (0..3600; default: 30)')
    parser.add_argument('--reported-track-max-longer-seconds', type=int, choices=range(0, 3601),
                        help='Maximum seconds the matched recording may be longer (0..3600; default: 30)')
    parser.add_argument('--ipc-counter',action='store_true',help='Enable PID-gated WebView ::ipc counting for idle-ipc only')
    parser.add_argument('--ipc-counter-shim',type=Path,help='Built private IPC counter shared object')
    parser.add_argument('--startup-diagnostics',action='store_true',
                        help='Opt in to safe startup/decoder diagnostics exported only after startup timing and pause')
    parser.add_argument('--startup-diagnostics-no-console-trace',action='store_true',
                        help='Export the same post-pause diagnostics without enabling application console trace flags')
    parser.add_argument('--schedstat-diagnostic',action='store_true',
                        help='Opt in to private unprivileged per-thread schedstat sampling for startup phase attribution')
    parser.add_argument('--controller',type=Path,default=Path('/tmp/burrata-queue-recovery-private-audio-controller.py'))
    parser.add_argument('--xvfb',type=Path,default=Path('/tmp/burrata-private-xvfb/root/usr/bin/Xvfb'))
    args=parser.parse_args()
    if (args.startup_diagnostics or args.startup_diagnostics_no_console_trace or args.schedstat_diagnostic) and args.mode!='startup':
        parser.error('startup diagnostics options are valid only with --mode startup')
    if sum((args.startup_diagnostics, args.startup_diagnostics_no_console_trace, args.schedstat_diagnostic)) > 1:
        parser.error('choose only one startup diagnostics option')
    reported_options = [args.preferred_streaming_source, args.fallback_sources,
                        args.disable_streaming_fallback, args.expected_source_provider]
    if args.reported_tracks:
        if args.mode != 'incidents':
            parser.error('--reported-tracks is supported only with --mode incidents')
        try:
            if args.reported_tracks.is_symlink():
                raise ValueError('--reported-tracks must not be a symlink')
            args.reported_tracks = args.reported_tracks.resolve(strict=True)
            if not args.reported_tracks.is_file():
                raise ValueError('--reported-tracks must be a regular file')
            args.fallback_sources = parse_streaming_fallback_sources(
                args.fallback_sources, args.preferred_streaming_source or 'youtube')
        except (OSError, ValueError) as error:
            parser.error(str(error))
        if args.reported_track_diagnostics and args.mode != 'incidents':
            parser.error('--reported-track-diagnostics is supported only with --mode incidents')
    elif args.reported_track_diagnostics:
        parser.error('--reported-track-diagnostics requires --reported-tracks')
    elif (any(value not in (None, False) for value in reported_options)
          or args.reported_track_startup_timeout_seconds != 20 or args.reported_track_pcm_window_seconds != 6
          or args.reported_track_matching_level is not None or args.reported_track_duration_policy is not None
          or args.reported_track_max_shorter_seconds is not None or args.reported_track_max_longer_seconds is not None):
        parser.error('reported-track provider/window/matching options require --reported-tracks')
    if args.mode=='idle-ipc':
        if not args.ipc_counter or not args.ipc_counter_shim:
            parser.error('--mode idle-ipc requires --ipc-counter and --ipc-counter-shim')
        args.ipc_counter_shim=args.ipc_counter_shim.resolve(strict=True)
        if not args.ipc_counter_shim.is_file():
            parser.error('--ipc-counter-shim must be a regular file')
    elif args.ipc_counter or args.ipc_counter_shim:
        parser.error('--ipc-counter options are valid only with --mode idle-ipc')
    payload=args.payload.resolve(strict=True);binary=payload/'usr/bin/burrata'
    appimage=None
    if args.appimage:
        try:appimage=validate_appimage_pin(args.appimage,payload,args.appimage_sha256,args.appimage_elf_sha256)
        except (OSError,ValueError) as error:parser.error(str(error))
    ui_actor_evidence=verify_private_ui_actor() if args.mode=='startup' else None
    resources=next(payload.glob('usr/lib/*/resources'))
    module=importlib.util.spec_from_file_location('private_audio',args.controller)
    audio=importlib.util.module_from_spec(module);module.loader.exec_module(audio)
    if audio.running_state().get('status')!='ready-private-no-sinks':raise RuntimeError('Private audio controller must already be ready')
    scratch_root, capture_root = scratch_roots()
    profile=Path(tempfile.mkdtemp(prefix='burrata-launch.',dir='/tmp'))
    ipc_counter_path=profile/'ipc-counter.json' if args.mode=='idle-ipc' else None
    settings=seed(profile,resources,args.mode,args)
    pending_count=seed_pending_quality_assets(profile,args.pending_quality_assets)
    # The app intentionally unpacks managed helpers in its data directory,
    # overriding TMPDIR. Put disposable data on the workspace filesystem too.
    scratch=scratch_root/profile.name;scratch.mkdir(parents=True,exist_ok=True)
    shutil.move(str(profile/'data'),str(scratch/'data'))
    (profile/'data').symlink_to(scratch/'data',target_is_directory=True)
    args.evidence.mkdir(parents=True,exist_ok=True)
    children=[];result={'mode':args.mode,'buildProfile':args.build_profile,'profile':str(profile),'binarySha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
        'appimagePath':str(appimage) if appimage else None,'appimageSha256':args.appimage_sha256 if appimage else None,
        'qualification':('Diagnostic dev build with custom protocol; not release qualification. ' if args.build_profile=='diagnostic' else '')+('verified AppImage payload; private virtual output; physical hardware and OS install not tested' if appimage else 'Extracted DEB payload; private virtual output; physical hardware and OS install not tested'),
        'settings':settings,'pendingQualityAssets':pending_count,'hostBefore':host_baseline(audio)}
    if ui_actor_evidence:result['privateGuiActionActor']=ui_actor_evidence
    if args.reported_tracks:
        runtime_cases=json.loads((profile/'fixture/runtime-cases.json').read_text())
        options=runtime_cases['reportedTrackOptions']
        result['reportedTrackQualification']={**options,'tracks':[{'id':track['id'],'title':track['title'],'artist':track['artist']}
            for track in runtime_cases['incidents']],
            'libraryRowLookupEnabled':args.reported_track_diagnostics,
            'playbackDiagnosticsEnabledInPrivateProfile':settings.get('playbackDiagnosticsEnabled') is True,
            'directTrackObjectPlaybackQualification':'not_run'}
    try:
        env=audio.private_env();sink='burrata_native_'+uuid.uuid4().hex[:8]
        module_id=run(['pactl','load-module','module-null-sink','sink_name='+sink,'rate=48000','channels=2'],env=env).stdout.decode().strip()
        env.update(XDG_DATA_HOME=str(profile/'data'),XDG_CONFIG_HOME=str(profile/'config'),XDG_CACHE_HOME=str(profile/'cache'),
            XDG_STATE_HOME=str(profile/'state'),BURRATA_APP_DATA=str(profile/'data/org.burrata.discovery'),
            BURRATA_ISOLATED_MUSIC_ROOT=str(profile/'fixture'),GDK_BACKEND='x11',PULSE_SINK=sink,NO_AT_BRIDGE='1',
            BURRATA_NATIVE_OUTPUT_QUALIFICATION='1',BURRATA_DB_METRICS='1')
        app_session_config, env, atspi_activation = startup_atspi_launch_settings(
            profile, args.mode, audio.DBUS_CONF, env)
        if atspi_activation is not None:
            result['privateAtspiActivation'] = atspi_activation
            result['privateAtspiEnvironment'] = {
                'noAtBridgeRemoved': 'NO_AT_BRIDGE' not in env,
                'xdgRuntimeDir': env['XDG_RUNTIME_DIR'],
                'privateBusForAppAndActor': True,
                'scope': 'startup AppImage and exact private AT-SPI actor only',
            }
        env.pop('WAYLAND_DISPLAY',None);env.pop('SESSION_MANAGER',None)
        alsa=profile/'config/alsa.conf';alsa.write_text('pcm.!default { type pulse server "'+env['PULSE_SERVER']+'" device "'+sink+'" }\nctl.!default { type pulse server "'+env['PULSE_SERVER']+'" }\n')
        env['ALSA_CONFIG_PATH']=str(alsa)
        scratch=scratch_root/profile.name;scratch.mkdir(parents=True,exist_ok=True)
        env['TMPDIR']=str(scratch)
        display=next(n for n in range(90,120) if not Path(f'/tmp/.X{n}-lock').exists() and not Path(f'/tmp/.X11-unix/X{n}').exists())
        env['DISPLAY']=':'+str(display)
        def launch(command,log,extra=None):
            child=subprocess.Popen([str(x) for x in command],env=extra or env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
            children.append(child);return child
        xvfb=launch([args.xvfb,env['DISPLAY'],'-screen','0','1280x900x24','-nolisten','tcp','-noreset'],
            (profile/'xvfb.log').open('wb'),dict(env,LD_LIBRARY_PATH=str(args.xvfb.parent.parent/'lib')))
        for _ in range(100):
            if xvfb.poll() is not None:raise RuntimeError('Private display exited')
            if Path(f'/tmp/.X11-unix/X{display}').exists():break
            time.sleep(.05)
        if args.mode=='malformed-aac':
            with sqlite3.connect(profile/'data/org.burrata.discovery/burrata.sqlite') as db:
                row=db.execute("select metadata from catalog_listings where metadata like '%Smoke AAC%'").fetchone()
            source=Path(json.loads(row[0])['localPath'])
            tools=resources/'tools/x86_64-unknown-linux-gnu'
            probe=run([tools/'ffprobe','-v','error','-show_entries','stream=duration:format=duration','-of','json',source],env=env)
            decoded=run([tools/'ffmpeg','-v','error','-i',source,'-ac','2','-ar','48000','-f','f32le','pipe:1'],env=env)
            if float(json.loads(probe.stdout)['format']['duration'])<23 or not 11.8<len(decoded.stdout)/8/48000<12.2:
                raise RuntimeError('Malformed AAC does not have independently verified contradictory duration')
            result['malformedReference']={'container':json.loads(probe.stdout),'decodedSeconds':len(decoded.stdout)/8/48000,
                'modifiedBoxes':json.loads((profile/'malformed-duration.json').read_text())}
        if args.mode=='aac':
            smoke_spec=importlib.util.spec_from_file_location('installed_tools',ROOT/'scripts/release/smoke-installed.py')
            smoke=importlib.util.module_from_spec(smoke_spec);smoke_spec.loader.exec_module(smoke)
            result['packagedTools']=smoke.bundled_tool_checks(resources,smoke.TARGETS['linux'],env)
            result['packagedMedia']=smoke.bundled_media_checks(resources,smoke.TARGETS['linux'],env,profile)
        raw_image_startup=args.mode=='startup' and appimage is not None
        app_launched_at_unix_ms=time.time()*1000
        app_launched_at_monotonic_ns=time.monotonic_ns()
        launch_target=appimage or binary
        app_command=['dbus-run-session','--config-file='+str(app_session_config),'--',launch_target]
        if raw_image_startup:
            launch_clock_receipt=profile/'app-launch-clock.json'
            launcher=PRIVATE_UI_ACTOR_DIR/'private_app_launcher.py'
            app_command=['dbus-run-session','--config-file='+str(app_session_config),'--',sys.executable,str(launcher),
                '--profile',str(profile),'--launch-clock-receipt',str(launch_clock_receipt),'--',str(launch_target)]
            app_environment=dict(env)
        else:
            app_environment=bind_app_launch_monotonic_origin(env,app_launched_at_monotonic_ns)
        if args.startup_diagnostics:
            app_environment=dict(env,BURRATA_STARTUP_TRACE='1',BURRATA_PLAYBACK_TRACE='1',BURRATA_PERF_TRACE='1')
        elif args.startup_diagnostics_no_console_trace or args.schedstat_diagnostic:
            app_environment=dict(env)
            for key in ('BURRATA_STARTUP_TRACE','BURRATA_PLAYBACK_TRACE','BURRATA_PERF_TRACE'):
                app_environment.pop(key,None)
        if args.mode=='idle-ipc':
            wrapper=profile/'launch-ipc-counter-target.sh'
            wrapper.write_text('#!/bin/sh\nBURRATA_IPC_COUNTER_PID=$$\nexport BURRATA_IPC_COUNTER_PID\nexec "$@"\n')
            wrapper.chmod(0o700)
            env['BURRATA_IPC_COUNTER_PATH']=str(ipc_counter_path.resolve())
            app_environment=dict(env)
            app_environment.update(BURRATA_IPC_COUNTER_ENABLE='1',BURRATA_IPC_COUNTER_PATH=str(ipc_counter_path.resolve()))
            app_environment.pop('BURRATA_IPC_COUNTER_PID',None)
            prior_preload=app_environment.get('LD_PRELOAD')
            app_environment['LD_PRELOAD']=str(args.ipc_counter_shim)+((' '+prior_preload) if prior_preload else '')
            if appimage:
                instrumented_apprun=profile/'AppRun-ipc-instrumented'
                result['appimageIpcInstrumentation']=instrument_appimage_apprun(payload,instrumented_apprun)
                app_environment['APPDIR']=str(payload)
                app_command=['dbus-run-session','--config-file='+str(audio.DBUS_CONF),'--','/bin/bash',str(instrumented_apprun)]
            else:
                app_command=['dbus-run-session','--config-file='+str(audio.DBUS_CONF),'--','/bin/sh',str(wrapper),str(launch_target)]
        app=launch(app_command,(profile/'app.log').open('wb'),app_environment)
        result['appLaunchedAtMonotonicNs']=None if raw_image_startup else app_launched_at_monotonic_ns
        for _ in range(300):
            if app.poll() is not None:raise RuntimeError('Packaged app exited')
            if (profile/'data/org.burrata.discovery/automation.json').exists():break
            time.sleep(.1)
        else:raise RuntimeError('Automation discovery timeout')
        result['appLaunchedAtUnixMs']=app_launched_at_unix_ms
        result['automationReadyAtUnixMs']=time.time()*1000
        if args.mode in ['mpris','source-audition','idle-ipc','startup']:
            raw_appimage_launch=appimage is not None and args.mode!='idle-ipc'
            app_pid,app_executable=_private_app_from_tree(app.pid,binary,file_sha256(binary),
                appimage if raw_appimage_launch else None,profile)
            if raw_image_startup:
                receipt=validate_appimage_launch_clock_receipt(profile,appimage,args.appimage_sha256,app_pid)
                app_launched_at_monotonic_ns=receipt['appLaunchMonotonicNs']
                app_launched_at_unix_ms=receipt['appLaunchUnixMs']
                env['BURRATA_QUAL_APP_LAUNCH_MONOTONIC_NS']=str(app_launched_at_monotonic_ns)
                result['appLaunchedAtUnixMs']=app_launched_at_unix_ms
                result['appLaunchClockReceipt']={'path':str(profile/'app-launch-clock.json'),
                    'nativeAppPid':app_pid,'appImagePid':receipt['appImagePid'],'launcherPid':receipt['launcherPid'],
                    'timestampScope':'raw AppImage Popen; excludes private AT-SPI registry setup and includes fresh FUSE startup'}
                result['appLaunchedAtMonotonicNs']=app_launched_at_monotonic_ns
            entries=Path(f'/proc/{app_pid}/environ').read_bytes().split(b'\0')
            selected={entry.split(b'=',1)[0]:entry.split(b'=',1)[1] for entry in entries if entry.startswith((b'DBUS_SESSION_BUS_ADDRESS=',b'XDG_DATA_HOME=',b'BURRATA_IPC_COUNTER_PID=',b'BURRATA_IPC_COUNTER_PATH='))}
            if selected.get(b'XDG_DATA_HOME',b'').decode()!=str(profile/'data'):raise RuntimeError('Private app profile mismatch')
            if args.mode=='idle-ipc':
                counter_pid=selected.get(b'BURRATA_IPC_COUNTER_PID',b'').decode()
                counter_file=selected.get(b'BURRATA_IPC_COUNTER_PATH',b'').decode()
                result['ipcCounterTargetVerified']=counter_pid==str(app_pid) and counter_file==str(ipc_counter_path.resolve())
                result['ipcCounterPath']=counter_file
                if not result['ipcCounterTargetVerified']:
                    raise RuntimeError('IPC counter PID/path is not bound to the private Burrata process')
            address=selected.get(b'DBUS_SESSION_BUS_ADDRESS',b'').decode()
            if not address.startswith('unix:path=/tmp/dbus-'):raise RuntimeError('App is not on a verified private bus')
            env['BURRATA_QUAL_PRIVATE_DBUS']=address;result['privateMprisBusVerified']=True
            env['BURRATA_QUAL_PRIVATE_APP_PID']=str(app_pid)
            env['BURRATA_QUAL_PRIVATE_APP_EXE']=str(app_executable)
            env['BURRATA_QUAL_PRIVATE_XVFB_PID']=str(xvfb.pid)
            if args.mode=='startup':
                env['BURRATA_QUAL_UI_ACTOR_SCRIPT']=ui_actor_evidence['entryPoint']
                env['BURRATA_QUAL_UI_ACTOR_PYTHON']=sys.executable
                env['BURRATA_QUAL_UI_ACTOR_PYTHONPATH']=ui_actor_evidence['supportDirectory']
        result['privateProbe']=audio.probe_private(env)
        if result['privateProbe']['hardwareObjects']:raise RuntimeError('Private server exposed hardware')
        capture=capture_root/(profile.name+'.f32le')
        if args.mode!='idle-ipc':
            result['capturePath']=str(capture);capture_file=capture.open('wb')
            recorder=subprocess.Popen(['parec','--device='+sink+'.monitor','--format=float32le','--rate=48000','--channels=2','--latency-msec=20'],
                env=env,stdout=capture_file,stderr=(profile/'recorder.log').open('wb'),start_new_session=True)
            children.append(recorder)
            result['captureStartedAtUnixMs']=time.time()*1000
            time.sleep(.3)
        schedstat_sampler=None
        schedstat_output=profile/'schedstat-samples.json'
        schedstat_markers=profile/'schedstat-markers.ndjson'
        if args.schedstat_diagnostic:
            app_pid,schedstat_targets=_schedstat_app_targets(app.pid,binary,
                appimage if appimage is not None and args.mode!='idle-ipc' else None,profile)
            audio_state=audio.running_state()
            if audio_state.get('status')!='ready-private-no-sinks':
                raise RuntimeError('Private audio controller state changed before schedstat sampling')
            audio_role={'pipewire':'pipewire','wireplumber':'wireplumber','pipewire-pulse':'pipewire-pulse'}
            service={'pid':audio_state.get('servicePid'),'exe':audio_state.get('serviceExe'),
                     'startTime':audio_state.get('serviceStartTime')}
            if not audio.process_matches_record(service):
                raise RuntimeError('Private audio controller identity changed before schedstat sampling')
            schedstat_targets.append({'role':'audio-controller','pid':service['pid']})
            for child in audio_state.get('children',[]):
                role=audio_role.get(child.get('name'))
                if role and audio.process_matches_record(child):
                    schedstat_targets.append({'role':role,'pid':child['pid']})
            schedstat_targets.append({'role':'parec','pid':recorder.pid})
            sampler_env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'PYTHONNOUSERSITE':'1'}
            schedstat_sampler=subprocess.Popen(
                [sys.executable,ROOT/'scripts/performance/native_schedstat_sampler.py',
                 '--output',schedstat_output,'--markers',schedstat_markers,'--interval-ms','10'],
                cwd=ROOT,env=sampler_env,stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,
                stderr=(profile/'schedstat-sampler.log').open('wb'),start_new_session=True)
            children.append(schedstat_sampler)
            schedstat_sampler.stdin.write(json.dumps(schedstat_targets).encode('utf-8'))
            schedstat_sampler.stdin.close()
            result['schedstatDiagnostic']={'scope':'private per-thread schedstat startup diagnostic only',
                'samplingIntervalMs':10,'targetRoles':sorted({target['role'] for target in schedstat_targets}),
                'appProcessIdentityVerified':_proc_executable(app_pid)==binary.resolve(),
                'privateAudioProcessCount':sum(target['role']=='audio-controller' or target['role'] in audio_role.values() for target in schedstat_targets),
                'captureRecorderIncluded':True,'webkitAndXvfbExcludedFromHotLoop':True,
                'processGlobalCpuPressureIncluded':True}
        mcp=resources/'tools/x86_64-unknown-linux-gnu/burrata-mcp'
        mode='aac' if args.mode=='malformed-aac' else 'incidents' if args.mode in ['broken-download','recovery-baseline','broken-playable','decoder-recovery','nipsey'] else args.mode
        if mode=='performance':
            result['profilerStartUnixMs']=time.time()*1000
            profiler=launch(['python3',ROOT/'scripts/performance/profile_process.py','--pid',str(app.pid),
                '--seconds','58','--interval','.5','--output',profile/'process-profile'],(profile/'profiler.log').open('wb'))
        if args.mode=='aac':
            gate=subprocess.run(['node',str(ROOT/'scripts/release/check-installed-native-audio.mjs'),str(mcp),
                str(profile/'data/org.burrata.discovery/automation.json'),json.loads((profile/'fixture/runtime-cases.json').read_text())['aacId']],
                cwd=ROOT,env=env,text=True,capture_output=True,timeout=100)
            (profile/'native-content-gate.txt').write_text(gate.stdout+gate.stderr)
            result['nativeContentGatePassed']=gate.returncode==0
        control_command=['node',str(ROOT/'scripts/performance/native_runtime_capture.mjs'),str(profile),str(mcp),mode]
        if args.startup_diagnostics:
            control_command.append('--startup-diagnostics')
        elif args.startup_diagnostics_no_console_trace:
            control_command.append('--startup-diagnostics-no-console-trace')
        if args.schedstat_diagnostic:
            control_command.append('--schedstat-diagnostic')
            env['BURRATA_SCHEDSTAT_MARKER_PATH']=str(schedstat_markers)
        if args.reported_track_diagnostics:
            control_command.append('--reported-track-diagnostics')
        control=subprocess.run(control_command,
            cwd=ROOT,env=env,text=True,capture_output=True,timeout=900 if mode=='source-audition' else 960 if mode=='full-nipsey' else 560 if mode=='full-incidents' else 420 if mode=='incidents' else 120)
        (profile/'controller.txt').write_text(control.stdout+control.stderr)
        time.sleep(.4)
        if args.mode!='idle-ipc':
            if recorder.poll() is not None:raise RuntimeError('Output recorder exited before completion; capture is unqualified')
            recorder.terminate();recorder.wait(timeout=5);capture_file.close()
        result['runtime']=json.loads((profile/'runtime-result.json').read_text())
        if schedstat_sampler is not None:
            schedstat_sampler.terminate()
            schedstat_sampler.wait(timeout=5)
            if schedstat_output.is_file():
                scheduler=json.loads(schedstat_output.read_text())
                enrich_phase_intervals(scheduler,result['runtime'])
                result['schedstatDiagnostic']['sampler']=scheduler
                result['schedstatDiagnostic']['samplerExitCode']=schedstat_sampler.returncode
            else:
                result['schedstatDiagnostic']['samplerExitCode']=schedstat_sampler.returncode
                result['schedstatDiagnostic']['error']='sampler_output_missing'
        if args.startup_diagnostics or args.startup_diagnostics_no_console_trace:
            result['startupPerformanceTrace']={
                'environmentFlags':({'BURRATA_STARTUP_TRACE':'1','BURRATA_PLAYBACK_TRACE':'1','BURRATA_PERF_TRACE':'1'}
                    if args.startup_diagnostics else {'BURRATA_STARTUP_TRACE':None,'BURRATA_PLAYBACK_TRACE':None,'BURRATA_PERF_TRACE':None}),
                'consoleTraceEnvironmentEnabled':args.startup_diagnostics,
                'note':'Only fixed startup marks and quality-facts worker timing events are projected; raw app stderr is not copied into this field.',
                'events':safe_startup_performance_trace(profile/'app.log'),
            }
        if mode=='performance':
            profiler.wait(timeout=25)
            import numpy as np
            rows=json.loads((profile/'process-profile/samples.json').read_text())
            result['performance']=[]
            for window in result['runtime']['windows']:
                selected=[row for row in rows if row.get('kind')=='group' and window['startUnixMs']+500<=result['profilerStartUnixMs']+row['atSeconds']*1000<=window['endUnixMs']]
                result['performance'].append(dict(phase=window['phase'],samples=len(selected),
                    medianCpuPercentOneCore=float(np.median([row['cpuPercent'] for row in selected])),
                    p95CpuPercentOneCore=float(np.percentile([row['cpuPercent'] for row in selected],95)),
                    medianTreeRssBytes=int(np.median([row['rssBytes'] for row in selected]))))
        if mode=='migration':
            import numpy as np
            pcm=np.fromfile(capture,dtype='<f4');peak=float(np.max(np.abs(pcm))) if len(pcm) else 0
            result['pcm']={'peak':peak,'passed':len(pcm)>0 and peak<.000001}
            with sqlite3.connect(profile/'data/org.burrata.discovery/burrata.sqlite') as db:
                values={key:json.loads(value) for key,value in db.execute("SELECT key,value FROM settings WHERE key in ('playbackEngine.v1','nativePlaybackQueue.v1','nativePlaybackSession.v1')")}
            result['migrationPersistence']={'engine':values.get('playbackEngine.v1'),'entries':values.get('nativePlaybackQueue.v1',{}).get('entryIds'),'position':values.get('nativePlaybackSession.v1',{}).get('position')}
            result['pcm']['passed'] &= result['migrationPersistence']=={'engine':'native','entries':['legacy-first','legacy-second'],'position':5}
        elif mode.startswith('stereo-'):
            import numpy as np
            pcm=np.fromfile(capture,dtype='<f4');pcm=pcm[:len(pcm)//2*2].reshape(-1,2)
            result['pcm']=assess_stereo_markers(pcm,3 if mode=='stereo-seek-opus' else 5)
        elif mode=='source-audition':
            import numpy as np
            pcm=np.fromfile(capture,dtype='<f4');pcm=pcm[:len(pcm)//2*2].reshape(-1,2)
            test=result['runtime']
            def segment(first,last):
                start=max(0,int((test.get(first,result['captureStartedAtUnixMs'])-result['captureStartedAtUnixMs'])/1000*48000))
                end=min(len(pcm),int((test.get(last,result['captureStartedAtUnixMs'])-result['captureStartedAtUnixMs'])/1000*48000))
                return pcm[start:end]
            result['pcm']=assess_source_audition_pcm(segment('localStartedAtUnixMs','localPausedAtUnixMs'),segment('auditionStartedAtUnixMs','auditionPausedAtUnixMs'))
            result['auditionQueueCheckpoint']=assess_audition_queue_checkpoint(profile)
            result['runtime']['passed'] &= result['auditionQueueCheckpoint']['passed']
        elif mode=='full-nipsey':
            import numpy as np
            pcm=np.fromfile(capture,dtype='<f4');pcm=pcm[:len(pcm)//2*2].reshape(-1,2)
            result['runtime']['cases']=assess_full_recording_pcm(pcm,result['runtime']['cases'],result['captureStartedAtUnixMs'])
            result['runtime']['passed'] &= all(item['passed'] for item in result['runtime']['cases'])
            result['pcm']={'passed':all(item['passed'] for item in result['runtime']['cases']), 'frames':len(pcm)}
        elif mode=='live-analysis':
            import numpy as np
            pcm=np.fromfile(capture,dtype='<f4');pcm=pcm[:len(pcm)//2*2].reshape(-1,2)
            rms=[float(np.sqrt(np.mean(pcm[:,channel]**2))) if len(pcm) else 0 for channel in [0,1]]
            active=sum(float(np.sqrt(np.mean(pcm[i:i+4800]**2)))>.005 for i in range(0,len(pcm)-4800,4800))*.1
            result['pcm']={'rmsByChannel':rms,'activeSeconds':active,'passed':min(rms)>.01 and active>=20}
        elif mode=='idle-ipc':
            idle=result['runtime'].get('idleIpc') or {}
            buckets=idle.get('measuredBuckets') if isinstance(idle.get('measuredBuckets'),list) else []
            bucket_delta_total=sum(bucket.get('countDelta',-1) for bucket in buckets
                                   if isinstance(bucket,dict) and isinstance(bucket.get('countDelta'),int))
            measured_elapsed=sum(bucket.get('windowMs',0) for bucket in buckets
                                 if isinstance(bucket,dict) and isinstance(bucket.get('windowMs'),(int,float)))
            result['ipcCounterPassed']=bool(result.get('ipcCounterTargetVerified')
                and isinstance(idle.get('registeredBefore'),int) and idle['registeredBefore']>=1
                and idle.get('registeredAfter')==idle.get('registeredBefore')
                and idle.get('warmupBucketCount')==3 and idle.get('measuredBucketCount')==20
                and isinstance(idle.get('actualElapsedMs'),(int,float)) and idle['actualElapsedMs']>=20700
                and isinstance(idle.get('measuredElapsedMs'),(int,float)) and idle['measuredElapsedMs']>=18000
                and isinstance(idle.get('countDelta'),int) and idle['countDelta']>=0
                and isinstance(idle.get('countBefore'),int) and isinstance(idle.get('countAfter'),int)
                and idle['countAfter']>=idle['countBefore']
                and idle['countDelta']==bucket_delta_total==idle['countAfter']-idle['countBefore']
                and isinstance(idle.get('ratePerSecond'),(int,float))
                and abs(idle['ratePerSecond']-(idle['countDelta']/(idle['measuredElapsedMs']/1000)))<1e-9
                and isinstance(idle.get('measuredBuckets'),list) and len(idle['measuredBuckets'])==20
                and abs(idle['measuredElapsedMs']-measured_elapsed)<1e-6
                and all(isinstance(bucket.get('countDelta'),int) and bucket['countDelta']>=0
                        and isinstance(bucket.get('windowMs'),(int,float)) and bucket['windowMs']>=900
                        for bucket in idle.get('measuredBuckets',[])))
        elif mode not in ['incidents','radio','full-incidents']:
            start=max(0,(result['runtime'].get('mainPlaybackStartedAtUnixMs',result['captureStartedAtUnixMs'])-result['captureStartedAtUnixMs'])/1000)
            result['pcm']=summarize_capture(capture,'natural' if args.mode.startswith('normalization-') else args.mode,start)
            if args.mode.startswith('normalization-'):
                result['pcm']['normalization']=assess_normalization(result['pcm']['bins'],args.mode.split('-')[1])
                result['pcm']['passed'] &= result['pcm']['normalization']['passed']
        else:
            import numpy as np
            pcm=np.fromfile(capture,dtype='<f4');pcm=pcm[:len(pcm)//2*2].reshape(-1,2)
            if mode=='full-incidents':
                rms=float(np.sqrt(np.mean(pcm**2))) if len(pcm) else 0
                active=sum(float(np.sqrt(np.mean(pcm[i:i+4800]**2)))>.0005 for i in range(0,len(pcm)-4800,4800))*.1
                wanted=sum(track['durationSeconds'] for track in json.loads((profile/'fixture/runtime-cases.json').read_text())['incidents'][1:])
                result['pcm']={'rms':rms,'activeSeconds':active,'expectedSuccessfulTrackSeconds':wanted,'passed':active>=wanted*.75}
            if mode=='radio':
                rms=float(np.sqrt(np.mean(pcm**2))) if len(pcm) else 0
                active=sum(float(np.sqrt(np.mean(pcm[i:i+4800]**2)))>.0005 for i in range(0,len(pcm)-4800,4800))*.1
                result['pcm']={'rms':rms,'activeSeconds':active,'passed':active>=45}
            reported_track_run=bool(args.reported_tracks and mode=='incidents')
            reported_track_options=(json.loads((profile/'fixture/runtime-cases.json').read_text())['reportedTrackOptions']
                if reported_track_run else None)
            for item in result['runtime']['cases'] if mode=='incidents' else []:
                if reported_track_run:
                    phase_start=item.get('pcmPhaseStartedAtUnixMs')
                    phase_end=item.get('pcmPhaseEndedAtUnixMs')
                    start=max(0,int((phase_start-result['captureStartedAtUnixMs'])/1000*48000)) if isinstance(phase_start,(int,float)) else 0
                    end=min(len(pcm),int((phase_end-result['captureStartedAtUnixMs'])/1000*48000)) if isinstance(phase_end,(int,float)) else start
                else:
                    start=max(0,int((item['startedAtUnixMs']-result['captureStartedAtUnixMs'])/1000*48000))
                    end=min(len(pcm),start+int(item['elapsedMs']/1000*48000))
                block=pcm[start:end]
                item['capturedRms']=float(np.sqrt(np.mean(block**2))) if len(block) else 0
                item['capturedActiveSeconds']=sum(float(np.sqrt(np.mean(block[i:i+4800]**2)))>.0005 for i in range(0,len(block)-4800,4800))*.1
                if reported_track_run:
                    gates=reported_track_case_gate(item['passed'],item['capturedActiveSeconds'],
                        bool(len(block)>0 and np.isfinite(block).all()),item.get('sourceProvider'),
                        item.get('expectedSourceProvider'),item.get('startupElapsedMs'),
                        reported_track_options['startupTimeoutSeconds']*1000,
                        item.get('pcmPhaseElapsedMs'),reported_track_options['pcmPhaseWindowSeconds']*1000)
                    item.update(gates)
                else:
                    item['capturedPcmPassed']=bool(len(block)>0 and np.isfinite(block).all() and item['capturedActiveSeconds']>=4.5)
                    item['passed']=bool(item['passed'] and item['capturedPcmPassed'])
            if reported_track_run:
                active=sum(item.get('capturedActiveSeconds',0) for item in result['runtime']['cases'])
                expected_count=len(json.loads((profile/'fixture/runtime-cases.json').read_text())['incidents'])
                result['pcm']={'scope':'private OS monitor PCM at final application output',
                    'minimumActiveSecondsPerTrack':4.5,
                    'pcmPhaseMinimumMs':5000,
                    'startupTimeoutSeconds':reported_track_options['startupTimeoutSeconds'],
                    'pcmPhaseWindowSeconds':reported_track_options['pcmPhaseWindowSeconds'],
                    'cases':[{'trackId':item.get('trackId'),'capturedActiveSeconds':item.get('capturedActiveSeconds'),
                        'capturedRms':item.get('capturedRms'),'startupPhasePassed':item.get('startupPhasePassed') is True,
                        'pcmPhasePassed':item.get('pcmPhasePassed') is True,'passed':item.get('capturedPcmPassed') is True}
                        for item in result['runtime']['cases']],
                    'aggregateCapturedActiveSeconds':active,
                    'passed':len(result['runtime']['cases'])==expected_count
                        and all(item.get('capturedPcmPassed') is True for item in result['runtime']['cases'])}
            if mode=='incidents':result['runtime']['passed']=incident_cases_passed(result['runtime'])
        if args.mode=='decoder-recovery':
            result['decoderFaultInjected']=(scratch/'decoder-fault-injected').exists()
            recovered=result['runtime']['cases'][0] if result['runtime']['cases'] else {}
            result['maximumRecoveryObserved']=max((sample.get('recoveryCount',0) for sample in result['runtime']['samples']),default=0)
            result['runtime']['passed'] &= result['decoderFaultInjected'] and result['maximumRecoveryObserved']>0 and not recovered.get('receiptError')
        result['passed']=control.returncode==0 and result['runtime']['passed'] and result.get('pcm',{'passed':True})['passed'] and result.get('nativeContentGatePassed',True)
        if mode=='idle-ipc':result['passed'] &= result.get('ipcCounterPassed') is True
        result['hostAfter']=host_baseline(audio)
        before,after=result['hostBefore'],result['hostAfter']
        result['hostInventoryChanged']=before.get('sinkNamesSha256')!=after.get('sinkNamesSha256')
        if not all(item.get('available') and item.get('inventoryReadable') and not item.get('testSinkPresent') for item in [before,after]) or before.get('defaultSink')!=after.get('defaultSink'):
            raise RuntimeError('Host default output or test isolation changed')
    except Exception as error:
        result['passed']=False
        result['error']='reported_capture_failed' if args.reported_tracks else str(error)
    finally:
        for child in reversed(children):
            if child.poll() is None:
                os.killpg(child.pid,signal.SIGTERM)
                try:child.wait(timeout=5)
                except subprocess.TimeoutExpired:os.killpg(child.pid,signal.SIGKILL);child.wait(timeout=3)
        if 'module_id' in locals():run(['pactl','unload-module',module_id],env=env)
        (args.evidence/(args.mode+'.json')).write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps({key:result.get(key) for key in ['mode','passed','profile','error','binarySha256']}),flush=True)
    return 0 if result['passed'] else 1


if __name__=='__main__':sys.exit(main())
