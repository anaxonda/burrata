#!/usr/bin/env python3
"""Fault-inject an AppImage in a disposable muted Linux profile.

Requires a graphical session, PulseAudio/PipeWire, dbus-run-session, gdbus,
and Node. All controls and signals are scoped to the spawned test instance;
the ordinary Burrata profile and its playback are never controlled.
Runs sequentially; local synthetic fixtures require no provider requests.
"""
import os, sys, json, time, signal, subprocess, sqlite3, tempfile, hashlib, urllib.request, urllib.parse, shutil
from pathlib import Path
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/performance'))
import run_isolated_memory as seed
import argparse
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--appimage', required=True, type=Path)
parser.add_argument('--output', required=True, type=Path)
args = parser.parse_args()
app = args.appimage.resolve(strict=True)
label = 'redteam-' + str(os.getpid())
out = args.output.resolve()
out.mkdir(parents=True, exist_ok=False)
seed.ROWS = 0
seed.FIXTURE_SECONDS = 24
profile = Path(tempfile.mkdtemp(prefix='burrata-launch.', dir='/tmp'))
seed.seed_profile(profile)
import uuid, wave
settings = dict(previewDelayMs=250, theme='dark', ytDlpPath='yt-dlp', ffmpegPath='ffmpeg', ffprobePath='ffprobe', jsRuntimePath='node', musicDirectory=str(profile / 'fixture/Music'), autoPreview=False, liveBpmEnabled=False, playbackDiagnosticsEnabled=True, preferredStreamingSource='youtube', streamingFallbackEnabled=False, backgroundSourcePreparation=False, matchingLevel='balanced', playlistMirroringEnabled=False, bandcampFallbackEnabled=False)
tracks = {}
with sqlite3.connect(profile / 'data/org.burrata.discovery/burrata.sqlite') as db:
    original = json.loads(db.execute("select metadata from catalog_listings where provider='local' limit 1").fetchone()[0])
    tracks['A'] = original
    for name, seconds, kind in [('ffmpeg', 40, 'valid'), ('B', 26, 'valid'), ('C', 28, 'valid'), ('short', 2, 'valid'), ('corrupt', 24, 'corrupt'), ('missing', 24, 'missing'), ('empty', 24, 'empty')]:
        path = profile / 'fixture' / (name + ('.m3u8' if name == 'ffmpeg' else '.wav'))
        if kind == 'valid':
            with wave.open(str(path), 'wb') as w:
                w.setnchannels(1)
                w.setsampwidth(2)
                w.setframerate(22050)
                w.writeframes(b'\x00' * (22050 * 2 * seconds))
        else:
            path.write_bytes(b'not audio ' + name.encode() if kind != 'empty' else b'')
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        size = path.stat().st_size
        tid = 'local:' + digest
        asset = str(uuid.uuid4())
        t = seed.track_record(tid, 'local', digest, 'Redteam ' + name, 'Burrata Synthetic Test', seconds, str(path), asset, 'wav')
        tracks[name] = t
        db.execute('INSERT INTO works(id,title,artist) VALUES(?,?,?)', ('work:' + tid, t['title'], t['artist']))
        db.execute('INSERT INTO recordings(id,work_id,version) VALUES(?,?,?)', (tid, 'work:' + tid, t['version']))
        db.execute('INSERT INTO catalog_listings(id,provider,provider_id,recording_id,source_url,metadata,updated_at) VALUES(?,?,?,?,?,?,?)', (tid, 'local', digest, tid, t['sourceUrl'], json.dumps(t), '2026-11-14T00:00:00Z'))
        payload = dict(id=asset, recordingId=tid, path=str(path), hash=digest, sourceCodec='wav', finalCodec='wav', sampleRate=22050, channels=1, bitDepth=16, durationSeconds=seconds, sizeBytes=size, bitrateOrigin='format', processing='existing-local-file', track=t)
        db.execute('INSERT INTO local_assets(id,recording_id,path,hash,payload) VALUES(?,?,?,?,?)', (asset, tid, str(path), digest, json.dumps(payload)))
        if kind == 'missing':
            path.unlink()
    for k, v in [('application', settings), ('automation.permissions.v1', dict(read=True, control=True, manage=False))]:
        db.execute('INSERT INTO settings(key,value) VALUES(?,?)', (k, json.dumps(v)))
env = os.environ.copy()
env['LC_ALL'] = 'C'
sink = 'burrata_resource_' + label
run = lambda args: subprocess.check_output(args, env=env, text=True).strip()
server = next((l.split(': ', 1)[1] for l in run(['pactl', 'info']).splitlines() if l.startswith('Server String: ')))
assert server.startswith('/') or server.startswith('unix:')
server = server if server.startswith('unix:') else 'unix:' + server
module = None
process = None
result = {'label': label, 'appImage': str(app), 'appPid': None, 'outcome': 'pending'}
with app.open('rb') as f:
    result['appImageSha256'] = hashlib.file_digest(f, 'sha256').hexdigest()

def save():
    (out / 'case.json').write_text(json.dumps(result, indent=2) + '\n')

def terminate_test(signum, frame):
    raise KeyboardInterrupt('Playback fault test interrupted')

signal.signal(signal.SIGTERM, terminate_test)
try:
    module = run(['pactl', 'load-module', 'module-null-sink', 'sink_name=' + sink, 'rate=44100', 'channels=2'])
    assert module.isdecimal()
    subprocess.run(['pactl', 'set-sink-mute', sink, '1'], env=env, check=True)
    (profile / 'config/alsa.conf').write_text('pcm.!default { type pulse server "' + server + '" device "' + sink + '" }\nctl.!default { type pulse server "' + server + '" }\n')
    data = str(profile / 'data/org.burrata.discovery')
    env.update(XDG_CONFIG_HOME=str(profile / 'config'), XDG_DATA_HOME=str(profile / 'data'), XDG_CACHE_HOME=str(profile / 'cache'), XDG_STATE_HOME=str(profile / 'state'), TMPDIR=str(profile / 'tmp'), BURRATA_APP_DATA=data, BURRATA_ISOLATED_MUSIC_ROOT=str(profile / 'fixture/Music'), ALSA_CONFIG_PATH=str(profile / 'config/alsa.conf'), PULSE_SERVER=server, PULSE_SINK=sink, PULSE_SOURCE=sink + '.monitor')
    stream_identity = 'org.burrata.muted-test.' + profile.name
    env['PULSE_PROP'] = 'application.name=Burrata-Muted-Test application.id=' + stream_identity + ' module-stream-restore.id=' + stream_identity
    with (out / 'app.log').open('wb') as f:
        process = subprocess.Popen(['dbus-run-session', '--', str(app)], env=env, stdin=subprocess.DEVNULL, stdout=f, stderr=f, start_new_session=True)
    discovery = profile / 'data/org.burrata.discovery/automation.json'
    deadline = time.monotonic() + 75
    while not discovery.exists():
        assert process.poll() is None and time.monotonic() < deadline, 'candidate startup failed'
        time.sleep(0.25)
    d = json.loads(discovery.read_text())
    url = urllib.parse.urlsplit(d['url'])
    assert url.hostname in ('127.0.0.1', 'localhost', '::1')
    endpoint = urllib.parse.urlunsplit((url.scheme, url.netloc, '/automation/call', '', ''))

    def call(name, args={}):
        req = urllib.request.Request(endpoint, data=json.dumps({'name': name, 'arguments': args}).encode(), headers={'Authorization': 'Bearer ' + d['token'], 'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=65) as f:
            r = json.load(f)
        assert not r.get('isError'), name + ' failed: ' + str(r.get('content', []))[:400]
        return r['structuredContent']['result']
    matches = []
    for p in Path('/proc').iterdir():
        if not p.name.isdecimal():
            continue
        try:
            exe = (p / 'exe').resolve(strict=True)
            if str(exe).endswith('/usr/bin/burrata') and b'BURRATA_APP_DATA=' + data.encode() in (p / 'environ').read_bytes().split(b'\x00'):
                matches.append((int(p.name), exe))
        except OSError:
            pass
    assert len(matches) == 1, 'exact test GUI process required'
    pid, exe = matches[0]
    result['appPid'] = pid
    with exe.open('rb') as f:
        result['executableSha256'] = hashlib.file_digest(f, 'sha256').hexdigest()
    with (out / 'readiness.log').open('wb') as f:
        subprocess.run(['node', str(ROOT / 'scripts/release/check-installed-mcp.mjs'), str(ROOT / 'scripts/dev/mcp-stdio.sh'), str(discovery)], stdout=f, stderr=f, check=True, timeout=60, cwd=ROOT)
    call('set_volume', {'volume': 0})
    assert call('playback_state', {'limit': 1})['volume'] == 0
    result['startedAndRemainedMuted'] = True
    if shutil.which('i3-msg'):
        tree = json.loads(run(['i3-msg', '-t', 'get_tree']))
        windows = []

        def visit(node):
            if node.get('pid') == pid:
                windows.append(node)
            for child in node.get('nodes', []) + node.get('floating_nodes', []):
                visit(child)
        visit(tree)
        assert len(windows) == 1
        subprocess.run(['i3-msg', '[con_id=' + str(windows[0]['id']) + ' pid=' + str(pid) + '] move scratchpad'], env=env, check=True, capture_output=True, timeout=5)
        result['windowVisibility'] = 'hidden in scratchpad'
    app_env = dict((part.split(b'=', 1) for part in Path('/proc/' + str(pid) + '/environ').read_bytes().split(b'\x00') if b'=' in part))
    address = app_env[b'DBUS_SESSION_BUS_ADDRESS'].decode()
    assert address.startswith('unix:') and address != os.environ.get('DBUS_SESSION_BUS_ADDRESS')

    def media(method, *args):
        return subprocess.check_output(['gdbus', 'call', '--address', address, '--dest', 'org.mpris.MediaPlayer2.burrata', '--object-path', '/org/mpris/MediaPlayer2', '--method', method, *args], text=True, timeout=10)

    def control(name):
        return media('org.mpris.MediaPlayer2.Player.' + name)

    def repeat(mode):
        return media('org.freedesktop.DBus.Properties.Set', 'org.mpris.MediaPlayer2.Player', 'LoopStatus', '<"' + mode + '">')

    def state():
        x = call('playback_state', {'limit': 20})
        assert x['volume'] == 0, 'Test must remain muted'
        return x

    def wait_for(predicate, seconds=8):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            x = state()
            if predicate(x):
                return x
            time.sleep(0.04)
        raise AssertionError('condition timeout: ' + json.dumps({k: x.get(k) for k in ('status', 'trackId', 'position', 'playerMessage', 'failure', 'queuedOperationId')}))

    def play(names):
        op = str(uuid.uuid4())
        call('play_tracks_queued', {'trackIds': [tracks[n]['id'] for n in names], 'operationId': op})
        return op

    def playing(name):
        return wait_for(lambda x: x['status'] == 'playing' and x['trackId'] == tracks[name]['id'])

    def case(name, fn):
        row = {'name': name, 'outcome': 'running'}
        result.setdefault('cases', []).append(row)
        save()
        start = time.monotonic()
        try:
            fn()
            row['outcome'] = 'passed'
        except Exception as e:
            row['outcome'] = 'failed'
            row['error'] = str(e)
        row['elapsedSeconds'] = round(time.monotonic() - start, 3)
        save()
        try:
            control('Stop')
            repeat('None')
        except Exception:
            pass

    def baseline():
        play(['A'])
        playing('A')
        time.sleep(0.2)
        call('pause_playback')
        x = wait_for(lambda x: x['status'] == 'paused')
        p = x['position']
        time.sleep(0.4)
        assert abs(state()['position'] - p) < 0.1
        call('resume_playback')
        playing('A')
    case('baseline_pause_resume', baseline)

    def isolated_route():
        play(['A'])
        playing('A')
        inputs = json.loads(run(['pactl', '--format=json', 'list', 'sink-inputs']))
        owned = [x for x in inputs if x.get('properties', {}).get('application.process.id') == str(pid)]
        assert len(owned) == 1 and owned[0]['properties'].get('application.id') == stream_identity
        sinks = json.loads(run(['pactl', '--format=json', 'list', 'sinks']))
        target = next((x for x in sinks if x['name'] == sink))
        assert owned[0]['sink'] == target['index']
        subprocess.run(['pactl', 'set-sink-input-mute', str(owned[0]['index']), '1'], env=env, check=True)
        result['actualSinkVerified'] = True
        result['audioClientIdentityIsolated'] = True
    case('verify_private_muted_route', isolated_route)

    def rapid_play():
        for _ in range(4):
            for name in ('A', 'B', 'C'):
                op = play([name])
        x = playing('C')
        assert x['queuedOperationId'] == op
        time.sleep(0.4)
        x = state()
        assert x['trackId'] == tracks['C']['id'] and x['status'] == 'playing'
    case('rapid_queue_replacement_latest_wins', rapid_play)

    def pause_loading():
        for _ in range(8):
            play(['A'])
            call('pause_playback')
            x = wait_for(lambda x: x['status'] == 'paused')
            p = x['position']
            time.sleep(0.08)
            assert state()['status'] == 'paused' and abs(state()['position'] - p) < 0.15
        call('resume_playback')
        playing('A')
    case('pause_while_starting', pause_loading)

    def seek_race():
        play(['A'])
        playing('A')
        for n in (18, 2, 21, 0):
            call('seek_playback', {'position': n})
        x = wait_for(lambda x: x['status'] == 'playing' and x['position'] < 3)
        time.sleep(0.3)
        assert state()['position'] < 4
    case('rapid_seeks_latest_wins', seek_race)

    def paused_seek():
        play(['A'])
        playing('A')
        call('pause_playback')
        call('seek_playback', {'position': 12})
        x = wait_for(lambda x: x['status'] == 'paused')
        time.sleep(0.3)
        assert state()['status'] == 'paused'
        call('resume_playback')
        x = playing('A')
        assert 11 <= x['position'] <= 15
    case('paused_seek_then_resume', paused_seek)

    def seek_pause():
        play(['A'])
        playing('A')
        call('seek_playback', {'position': 20})
        call('pause_playback')
        x = wait_for(lambda x: x['status'] == 'paused')
        time.sleep(0.3)
        assert state()['status'] == 'paused'
        call('resume_playback')
        playing('A')
    case('pause_during_seek', seek_pause)

    def bad_queue():
        play(['corrupt', 'missing', 'empty', 'B'])
        playing('B')
        x = state()
        assert x['currentIndex'] == 3 and x['diagnostics'].get('lastSkippedTrackFailure')
    case('invalid_missing_empty_then_valid', bad_queue)

    def all_bad():
        repeat('Playlist')
        play(['corrupt', 'missing', 'empty'])
        x = wait_for(lambda x: x['status'] in ('failed', 'ended', 'paused'), 15)
        revision = x['revision']
        time.sleep(0.6)
        x = state()
        assert x['status'] in ('failed', 'ended', 'paused')
        assert x['revision'] - revision < 8
    case('all_invalid_with_repeat_all_is_bounded', all_bad)

    def near_eof_seek():
        play(['A', 'B'])
        playing('A')
        call('seek_playback', {'position': 23.9})
        x = playing('B')
        assert x['currentIndex'] == 1
        time.sleep(0.4)
        assert state()['trackId'] == tracks['B']['id']
    case('seek_to_end_advances_once', near_eof_seek)

    def short_repeat():
        repeat('Track')
        play(['short'])
        x = playing('short')
        g = x['generation']
        x = wait_for(lambda x: x['generation'] > g and x['status'] == 'playing', 5)
        assert x['trackId'] == tracks['short']['id']
        repeat('None')
    case('short_track_repeat_one', short_repeat)

    def huge_seek():
        control('Stop')
        try:
            call('seek_playback', {'position': 1e+300})
        except AssertionError:
            pass
        state()
        play(['A'])
        playing('A')
        before = state()
        try:
            call('seek_playback', {'position': 1e+300})
        except AssertionError:
            pass
        else:
            raise AssertionError('Invalid seek was accepted')
        x = state()
        assert x['status'] == 'playing' and x['trackId'] == before['trackId'] and (x['generation'] == before['generation'])
    case('extreme_finite_seek_does_not_crash', huge_seek)

    def suspend_output():
        play(['A'])
        playing('A')
        subprocess.run(['pactl', 'suspend-sink', sink, '1'], env=env, check=True)
        try:
            time.sleep(1)
            call('pause_playback')
            wait_for(lambda x: x['status'] == 'paused')
        finally:
            subprocess.run(['pactl', 'suspend-sink', sink, '0'], env=env, check=True)
        call('resume_playback')
        playing('A')
    case('owned_output_suspend_pause_resume', suspend_output)

    def stop_seek():
        play(['A', 'B'])
        playing('A')
        call('seek_playback', {'position': 18})
        control('Stop')
        x = wait_for(lambda x: x['status'] == 'idle')
        time.sleep(0.5)
        x = state()
        assert x['status'] == 'idle' and x['position'] == 0
    case('stop_during_seek_stays_stopped', stop_seek)

    def next_seek():
        play(['A', 'B'])
        playing('A')
        call('seek_playback', {'position': 18})
        control('Next')
        x = playing('B')
        assert x['position'] < 3
        time.sleep(0.3)
        x = state()
        assert x['trackId'] == tracks['B']['id'] and x['position'] < 4
    case('next_during_seek_does_not_inherit_old_position', next_seek)

    def manual_next_repeat():
        repeat('Track')
        play(['A', 'B'])
        playing('A')
        control('Next')
        playing('B')
    case('manual_next_overrides_repeat_one', manual_next_repeat)

    def stop_preload():
        play(['short', 'B'])
        playing('short')
        control('Stop')
        time.sleep(2.4)
        assert state()['status'] == 'idle'
    case('stopped_queue_does_not_promote_preload', stop_preload)

    def pause_bad():
        for _ in range(5):
            play(['corrupt', 'B'])
            call('pause_playback')
            time.sleep(0.2)
            x = state()
            assert x['status'] in ('paused', 'failed'), x['status']
    case('pause_during_failed_entry_skip_does_not_autoplay', pause_bad)

    def duplicate_occurrences():
        play(['short', 'short', 'B'])
        x = playing('short')
        entry = x['entryId']
        x = wait_for(lambda x: x['trackId'] == tracks['short']['id'] and x['entryId'] != entry and (x['status'] == 'playing'), 5)
        assert x['currentIndex'] == 1
        playing('B')
    case('duplicate_recordings_have_distinct_queue_occurrences', duplicate_occurrences)

    def invalid_requests():
        play(['A'])
        playing('A')
        before = state()
        for method, args in [('seek_playback', {'position': -1}), ('seek_playback', {'position': 'bad'}), ('set_volume', {'volume': 2}), ('set_volume', {'volume': -1})]:
            try:
                call(method, args)
            except Exception:
                pass
            else:
                raise AssertionError('Invalid request accepted: ' + method)
            x = state()
            assert x['status'] == 'playing' and x['trackId'] == before['trackId']
    case('invalid_controls_leave_valid_playback_running', invalid_requests)

    def unknown_track():
        play(['A'])
        playing('A')
        try:
            call('play_tracks_queued', {'trackIds': ['local:nonexistent-redteam-id'], 'operationId': str(uuid.uuid4())})
        except AssertionError:
            pass
        else:
            raise AssertionError('Unknown track was accepted')
        time.sleep(0.4)
        x = state()
        assert x['status'] == 'playing' and x['trackId'] == tracks['A']['id']
    case('unknown_queue_request_does_not_stop_current_audio', unknown_track)

    def seek_after_end():
        play(['short'])
        playing('short')
        wait_for(lambda x: x['status'] == 'ended', 4)
        call('seek_playback', {'position': 1})
        x = wait_for(lambda x: x['status'] == 'paused')
        call('resume_playback')
        x = playing('short')
        assert 0.8 <= x['position'] <= 2
    case('seek_after_natural_end_resumes_selected_position', seek_after_end)

    def beyond_end():
        for value in (30, 1000, 1000000000000.0, 1e+18):
            play(['A'])
            playing('A')
            before = state()
            call('seek_playback', {'position': value})
            x = wait_for(lambda x: x['status'] in ('ended', 'failed', 'paused'), 4)
            if value in (30, 1000):
                assert x['status'] == 'ended' and x['position'] <= before['duration']
            if x['status'] == 'ended':
                assert x['duration'] <= before['duration']
            result.setdefault('beyondEnd', []).append({'requested': value, 'beforeDuration': before['duration'], 'afterStatus': x['status'], 'position': x['position']})
            save()
    case('seek_past_physical_eof', beyond_end)

    def decoder_identity(target):
        try:
            p = Path('/proc/' + str(target))
            v = (p / 'stat').read_text().rsplit(')', 1)[1].split()
            return (str((p / 'exe').resolve(strict=True)), int(v[1]), v[19])
        except (OSError, ValueError):
            return None

    def decoder_child():
        parents = {}
        candidates = []
        for p in Path('/proc').iterdir():
            if not p.name.isdecimal():
                continue
            value = decoder_identity(int(p.name))
            if value:
                parents[int(p.name)] = value
        owned = {pid}
        while True:
            more = {p for p, v in parents.items() if v[1] in owned}
            if more <= owned:
                break
            owned |= more
        for p in owned - {pid}:
            v = parents[p]
            if Path(v[0]).name == 'ffmpeg' and Path(v[0]).is_relative_to(exe.parents[2]):
                candidates.append((p, v))
        assert len(candidates) == 1, 'Expected one exact owned packaged decoder'
        return candidates[0]

    def held_decoder(pause_user=False):
        play(['ffmpeg'])
        before = playing('ffmpeg')
        target, identity = decoder_child()
        assert decoder_identity(target) == identity
        os.kill(target, signal.SIGSTOP)
        try:
            wait_for(lambda x: x['status'] == 'buffering', 6)
            if pause_user:
                call('pause_playback')
                wait_for(lambda x: x['status'] == 'paused')
            else:
                time.sleep(1)
        finally:
            if decoder_identity(target) == identity:
                os.kill(target, signal.SIGCONT)
        if pause_user:
            x = state()
            p = x['position']
            time.sleep(0.3)
            assert state()['status'] == 'paused' and abs(state()['position'] - p) < 0.15
            call('resume_playback')
        x = playing('ffmpeg')
        assert not x['diagnostics'].get('lastSkippedTrackFailure')
        assert x['trackId'] == before['trackId']
    case('owned_decoder_stall_recovers_same_recording', lambda: held_decoder(False))
    case('pause_during_decoder_stall_keeps_user_intent', lambda: held_decoder(True))

    def killed_decoder():
        play(['ffmpeg'])
        before = playing('ffmpeg')
        target, identity = decoder_child()
        assert decoder_identity(target) == identity
        os.kill(target, signal.SIGKILL)
        x = wait_for(lambda x: x['status'] == 'playing' and x['generation'] > before['generation'] and (x['trackId'] == before['trackId']), 15)
        assert x['diagnostics'].get('recoveryCount', 0) > 0 and (not x['diagnostics'].get('lastSkippedTrackFailure'))
        time.sleep(0.6)
        after = state()
        assert after['status'] == 'playing' and after['generation'] == x['generation'] and after['position'] > x['position'] + 0.1
        result['decoderRestart'] = {'previousGeneration': before['generation'], 'recoveredGeneration': after['generation'], 'recoveryCount': after['diagnostics'].get('recoveryCount'), 'initialPosition': x['position'], 'laterPosition': after['position']}
        save()
    case('killed_owned_decoder_restarts_same_recording', killed_decoder)

    def paused_decoder_death():
        play(['ffmpeg'])
        before = playing('ffmpeg')
        call('pause_playback')
        wait_for(lambda x: x['status'] == 'paused')
        target, identity = decoder_child()
        assert decoder_identity(target) == identity
        os.kill(target, signal.SIGKILL)
        time.sleep(0.3)
        assert state()['status'] == 'paused'
        call('resume_playback')
        result['resumeObservations'] = []
        for _ in range(20):
            x = state()
            result['resumeObservations'].append({'status': x['status'], 'position': x['position'], 'generation': x['generation'], 'recovery': x['diagnostics'].get('recoveryCount')})
            save()
            time.sleep(0.4)
        x = state()
        assert x['trackId'] == before['trackId'] and x['status'] == 'playing' and (x['generation'] > before['generation']), 'Resume did not replace the dead decoder'
        assert x['position'] > 0.5, 'Recovered decoder did not advance playback'
    case('decoder_death_while_paused_resumes_valid_audio', paused_decoder_death)

    def repeated_decoder_death_is_bounded():
        play(['ffmpeg'])
        before = playing('ffmpeg')
        for attempt in range(3):
            target, identity = decoder_child()
            assert decoder_identity(target) == identity
            os.kill(target, signal.SIGKILL)
            if attempt < 2:
                x = wait_for(lambda x: x['status'] == 'playing' and x['generation'] > before['generation'], 12)
                assert x['trackId'] == before['trackId'] and x['diagnostics'].get('recoveryCount') == attempt + 1
                before = x
            else:
                x = wait_for(lambda x: x['status'] == 'failed', 8)
                assert x['trackId'] == before['trackId']
                generation = x['generation']
                time.sleep(0.5)
                assert state()['status'] == 'failed' and state()['generation'] == generation
    case('repeated_decoder_death_has_finite_retries', repeated_decoder_death_is_bounded)
    call('pause_playback')
    report = call('get_playback_diagnostics_report')
    (out / 'playback-diagnostics.json').write_text(json.dumps(report, indent=2) + '\n')
    result['outcome'] = 'completed'
    result['passed'] = sum((x['outcome'] == 'passed' for x in result['cases']))
    result['failed'] = sum((x['outcome'] == 'failed' for x in result['cases']))
    save()
    print(json.dumps(result), flush=True)
except BaseException as e:
    result['outcome'] = 'failed'
    result['error'] = str(e)
    save()
    raise
finally:
    if process and process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
    if module:
        listing = run(['pactl', 'list', 'short', 'modules'])
        if any((l.startswith(module + '\tmodule-null-sink\t') and 'sink_name=' + sink in l for l in listing.splitlines())):
            subprocess.run(['pactl', 'unload-module', module], env=env, check=True)
    assert profile.parent == Path('/tmp') and profile.name.startswith('burrata-launch.') and ((profile / '.burrata-isolated-profile').read_text() == 'burrata isolated memory run\n')
    for _ in range(40):
        if not any((p.is_mount() for p in (profile / 'tmp').glob('.mount_*'))):
            break
        time.sleep(0.25)
    assert not any((p.is_mount() for p in (profile / 'tmp').glob('.mount_*'))), 'test mount still active'
    shutil.rmtree(profile)
if result.get('outcome') != 'completed' or result.get('failed') != 0:
    raise SystemExit('Playback fault cases failed; inspect case.json')
