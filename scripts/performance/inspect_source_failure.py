#!/usr/bin/env python3
"""Compare exact-source extraction without importing, downloading, or exporting URLs.

Run on the affected Mac with the source's public page URL and the bundled
helper. Optionally compare a second helper. This isolates extraction; it does
not reproduce Burrata's YouTube token broker or prove successful playback.
Private stderr is written only when --private-error-file is explicitly supplied;
keep that file local and out of diagnostics/issue attachments.
"""
import argparse
import json
import os
import subprocess
from urllib.parse import urlsplit


def reason(stderr):
    value = stderr.lower()
    for code, patterns in [
        ('tls_certificate', ['certificate verify failed', 'certificate_verify_failed']),
        ('geo_restricted', ['not available in your country', 'not available in your region']),
        ('subscription_required', ['soundcloud go+', 'subscription required']),
        ('provider_client', ['unable to extract client id', 'invalid client_id']),
        ('helper_exception', ['traceback (most recent call last)']),
        ('format_unavailable', ['requested format is not available']),
        ('access_denied', ['http error 403']),
        ('rate_limited', ['http error 429']),
    ]:
        if any(pattern in value for pattern in patterns):
            return code
    return 'other'


def inspect(helper, source, timeout):
    args = [helper, '--ignore-config', '--no-plugin-dirs', '--no-remote-components',
            '--no-color', '--dump-single-json', '--no-playlist', '--skip-download',
            '--format', 'bestaudio', '--', source]
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return {'ok': False, 'reason': 'timeout'}, ''
    except OSError:
        return {'ok': False, 'reason': 'helper_launch'}, ''
    report = {'ok': result.returncode == 0, 'exitCode': result.returncode}
    if result.returncode:
        report['reason'] = reason(result.stderr)
    else:
        try:
            data = json.loads(result.stdout)
            report.update(protocol=data.get('protocol'), codec=data.get('acodec'),
                          formatId=data.get('format_id'))
        except ValueError:
            report.update(ok=False, reason='invalid_json')
    return report, result.stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source')
    parser.add_argument('--helper', required=True)
    parser.add_argument('--comparison-helper')
    parser.add_argument('--timeout', type=int, default=20)
    parser.add_argument('--private-error-file')
    args = parser.parse_args()
    url = urlsplit(args.source)
    host = url.hostname or ''
    if url.scheme != 'https' or url.username or url.password or url.port not in (None, 443) or not any(
        host == domain or host.endswith('.' + domain) for domain in ('soundcloud.com', 'youtube.com', 'youtu.be')
    ):
        parser.error('Use a public HTTPS SoundCloud or YouTube page URL')
    reports, errors = {}, []
    for label, helper in [('bundled', args.helper), ('comparison', args.comparison_helper)]:
        if helper:
            reports[label], stderr = inspect(helper, args.source, args.timeout)
            errors.append(label + '\n' + stderr)
    if args.private_error_file:
        fd = os.open(args.private_error_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, 'w') as file:
            file.write('\n'.join(errors))
    print(json.dumps({'scope': 'extraction_only', 'results': reports}, indent=2))


if __name__ == '__main__':
    main()
