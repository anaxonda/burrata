#!/usr/bin/env python3
"""Run opt-in synthetic decoder faults, with no device or public-provider requests."""
import argparse
from contextlib import contextmanager, nullcontext
import os
import tempfile
import json
from pathlib import Path
import signal
import subprocess
import time

from playback_reliability import terminate_owned_tree


@contextmanager
def private_audio(output):
    with tempfile.TemporaryDirectory(prefix='burrata-network-audio-') as folder:
        root = Path(folder)
        socket = root / 'pulse.sock'
        sink = 'burrata_network_fault'
        with (output / 'private-audio.log').open('wb') as log:
            pulse = subprocess.Popen([
                'pulseaudio', '-n', '--daemonize=no', '--use-pid-file=no',
                '--exit-idle-time=-1', '--disable-shm=yes',
                '--load=module-null-sink sink_name=' + sink + ' rate=44100 channels=2',
                '--load=module-native-protocol-unix socket=' + str(socket) + ' auth-anonymous=1',
            ], stdout=log, stderr=log, start_new_session=True)
            try:
                deadline = time.monotonic() + 15
                while not socket.exists():
                    if pulse.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError('Private CI audio failed to start')
                    time.sleep(.05)
                server = 'unix:' + str(socket)
                subprocess.run(['pactl', '--server=' + server, 'set-sink-mute', sink, '1'], check=True, timeout=5)
                alsa = root / 'alsa.conf'
                alsa.write_text('pcm.!default { type pulse server "' + server + '" device "' + sink + '" }\nctl.!default { type pulse server "' + server + '" }\n')
                env = os.environ.copy()
                env.update(ALSA_CONFIG_PATH=str(alsa), PULSE_SERVER=server, PULSE_SINK=sink,
                           BURRATA_NETWORK_CONTROLLER_AUDIO='private-muted')
                yield env
            finally:
                pulse.terminate()
                try:
                    pulse.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pulse.kill()
                    pulse.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    artifacts = [json.loads(line) for line in args.artifacts.read_text().splitlines() if line.strip()]
    binaries = {Path(row['executable']).resolve(strict=True) for row in artifacts
                if row.get('reason') == 'compiler-artifact' and row.get('executable')
                and row.get('target', {}).get('name') == 'burrata_lib'
                and row.get('profile', {}).get('test')}
    if len(binaries) != 1:
        raise RuntimeError('Expected one exact Cargo library test executable')
    binary = str(binaries.pop())
    args.output.mkdir(parents=True, exist_ok=True)
    scenarios = [
        ('cached_seek_body_gap', 'native_output::tests::cached_rodio_seek_same_range_body_gap_starves_then_recovers', False, 90),
        ('paced_delivery_gap', 'native_output::tests::decoded_startup_reserve_independent_timeline_ab_bridges_bounded_body_gap', False, 120),
        ('tls_fault_recovery', 'native_output::remote_recovery_composition_tests::controlled_remote_tls_resolver_decoder_composition', True, 240),
        ('expired_source_fallback', 'native_output::remote_recovery_composition_tests::expired_stream_advances_to_valid_fallback_but_selected_source_stays_locked', True, 180),
    ]
    scenarios.append(('network_controller', 'native_output::remote_recovery_composition_tests::controller_fault_tests::network_faults_use_controller_recovery_and_preserve_user_intent', True, 240))
    rows = []
    for name, test, namespace, budget in scenarios:
        command = [binary, test, '--exact', '--ignored', '--nocapture', '--test-threads=1']
        if namespace:
            # This address belongs only to the child namespace. The fixture
            # asserts that lo is its sole interface and that no egress route exists.
            command = ['unshare', '--user', '--map-root-user', '--net', '--pid', '--fork', '--mount-proc', '--kill-child', '--', 'sh', '-eu', '-c',
                       'ip link set lo up\nip addr add 8.8.8.8/32 dev lo\nexec env BURRATA_REMOTE_FIXTURE_NAMESPACE=1 "$@"',
                       'playback-redteam', *command]
        row = {'name': name, 'test': test, 'networkNamespace': namespace, 'deadlineSeconds': budget}
        if name == 'network_controller':
            command = ['xvfb-run', '-a', 'dbus-run-session', '--', *command]
        began = time.monotonic()
        audio = private_audio(args.output) if name == 'network_controller' else nullcontext(None)
        with audio as env, (args.output / (name + '.log')).open('wb') as log:
            process = subprocess.Popen(command, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
            try:
                process.wait(timeout=budget)
                row['exitCode'] = process.returncode
            except subprocess.TimeoutExpired:
                # Includes helpers in independent process groups. Root inside
                # the user namespace still has this caller's UID on the host;
                # killing its PID-namespace init also reaps remaining children.
                owned = terminate_owned_tree(process.pid, signal.SIGTERM)
                time.sleep(.2)
                terminate_owned_tree(process.pid, signal.SIGKILL, owned)
                process.wait(timeout=10)
                row['timedOut'] = True
        text = (args.output / (name + '.log')).read_text(errors='replace')
        row['passed'] = row.get('exitCode') == 0 and 'test result: ok. 1 passed;' in text
        row['elapsedSeconds'] = round(time.monotonic() - began, 3)
        rows.append(row)
        (args.output / 'report.json').write_text(json.dumps({'scope': 'synthetic decoder/resolver and production playback-controller fixtures; private muted CI output; no public egress', 'cases': rows}, indent=2) + '\n')
    if not all(row['passed'] for row in rows):
        raise RuntimeError('One or more playback fault fixtures failed; inspect retained logs')


if __name__ == '__main__':
    main()
