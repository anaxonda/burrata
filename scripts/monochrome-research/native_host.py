#!/usr/bin/env python3
"""Local-only sink for sanitized Monochrome diagnostics. No browser secrets."""
import json
from pathlib import Path
import re
import struct
import sys

MAX_BYTES = 512 * 1024
DIAGNOSTIC_STAGES = {
    'monitor', 'tab_query', 'capture_injection', 'capture_read',
    'native_message', 'capture',
}
DIAGNOSTIC_CODES = {
    'starting', 'waiting_for_monochrome_tab', 'site_permission_missing', 'permission_denied', 'api_error',
    'native_host_unregistered', 'native_host_unavailable',
    'report_not_serializable', 'api_error', 'empty_report', 'saved',
    'host_rejected_report',
}


def write_status(root, state):
    """Write fixed, non-sensitive receiver diagnostics for local troubleshooting."""
    target = root / '.cache/monochrome-research-captures'
    target.mkdir(parents=True, exist_ok=True, mode=0o700)
    status = target / 'receiver-status.json'
    pending = target / 'receiver-status.tmp'
    pending.write_text(json.dumps({'state': state}, indent=2) + '\n')
    pending.chmod(0o600)
    pending.replace(status)


def validate_diagnostic(diagnostic):
    if (not isinstance(diagnostic, dict)
            or set(diagnostic) != {'stage', 'code'}
            or diagnostic.get('stage') not in DIAGNOSTIC_STAGES
            or diagnostic.get('code') not in DIAGNOSTIC_CODES):
        raise ValueError('Invalid diagnostic')
    return {'stage': diagnostic['stage'], 'code': diagnostic['code']}


def write_background_status(root, diagnostic):
    target = root / '.cache/monochrome-research-captures'
    target.mkdir(parents=True, exist_ok=True, mode=0o700)
    status = target / 'background-status.json'
    pending = target / 'background-status.tmp'
    pending.write_text(json.dumps(diagnostic, indent=2) + '\n')
    pending.chmod(0o600)
    pending.replace(status)


def validate(report):
    if not isinstance(report, dict) or report.get('schemaVersion') != 1:
        raise ValueError('Invalid report')
    if report.get('pageOrigin') != 'https://monochrome.samidy.com':
        raise ValueError('Unexpected origin')
    if report.get('evidenceKind') != 'page_observed_sanitized_network':
        raise ValueError('Unexpected evidence')
    def visit(value):
        if isinstance(value, str):
            if re.search(r'Bearer\s|eyJ[\w-]+\.[\w-]+\.|\b[a-fA-F0-9]{32,}\b|https?://[^\s]*[?#]', value):
                raise ValueError('Unsanitized value')
        elif isinstance(value, dict):
            for key, child in value.items():
                visit(key)
                visit(child)
        elif isinstance(value, list):
            for child in value:
                visit(child)
    visit(report)
    return report


def main():
    reply = {'ok': False}
    root = Path(__file__).resolve().parents[2]
    try:
        prefix = sys.stdin.buffer.read(4)
        if len(prefix) != 4:
            raise ValueError('Missing frame')
        length = struct.unpack('<I', prefix)[0]
        if not 0 < length <= MAX_BYTES:
            raise ValueError('Frame too large')
        data = sys.stdin.buffer.read(length)
        if len(data) != length:
            raise ValueError('Truncated frame')
        message = json.loads(data)
        if not isinstance(message, dict):
            raise ValueError('Invalid message')
        if set(message) == {'diagnostic'}:
            diagnostic = validate_diagnostic(message['diagnostic'])
            write_background_status(root, diagnostic)
            reply = {'ok': True}
        elif set(message) == {'report'}:
            report = validate(message['report'])
        else:
            raise ValueError('Invalid message')
        if 'report' in message:
            report = message['report']
        target = root / '.cache/monochrome-research-captures'
        if 'report' in message:
            target.mkdir(parents=True, exist_ok=True, mode=0o700)
            # Fixed destination; the extension cannot choose paths or run commands.
            output = target / 'latest.json'
            pending = target / 'latest.tmp'
            pending.write_text(json.dumps(report, indent=2) + '\n')
            pending.chmod(0o600)
            pending.replace(output)
            reply = {'ok': True}
            write_status(root, 'accepted')
    except Exception as error:
        # Fixed labels only; never log exception text or rejected content.
        if isinstance(error, ValueError):
            state = 'rejected'
        elif isinstance(error, (KeyError, TypeError, json.JSONDecodeError)):
            state = 'invalid_message'
        else:
            state = 'receiver_error'
        try:
            write_status(root, state)
        except Exception:
            pass
    encoded = json.dumps(reply).encode()
    sys.stdout.buffer.write(struct.pack('<I', len(encoded)) + encoded)
    sys.stdout.buffer.flush()


if __name__ == '__main__':
    main()
