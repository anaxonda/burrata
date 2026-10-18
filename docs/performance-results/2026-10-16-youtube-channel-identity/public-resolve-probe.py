#!/usr/bin/env python3
"""Bounded no-auth YouTube resolve_url shape probe. Never prints URL or IDs."""
import json, re, urllib.request, urllib.error, datetime, hashlib
UA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/131 Safari/537.36'
FIXED_WEB_VERSION = '2.20260708.00.00'
PUBLIC_HANDLE_URL = 'https://www.youtube.com/@YouTubeMusic'

def public_key(page_host):
    req = urllib.request.Request(f'https://{page_host}/', headers={'User-Agent': UA, 'Accept': 'text/html'})
    with urllib.request.urlopen(req, timeout=10) as response:
        html = response.read(1_000_000).decode('utf-8', 'replace')
    match = re.search(r'["\']INNERTUBE_API_KEY["\']\s*:\s*["\']([^"\']{10,160})["\']', html)
    if not match:
        raise RuntimeError('public page key unavailable')
    return match.group(1)

def summarize(body):
    endpoint = body.get('endpoint') or {}
    browse = endpoint.get('browseEndpoint') or body.get('browseEndpoint') or {}
    url_endpoint = endpoint.get('urlEndpoint') or {}
    raw = url_endpoint.get('url', '')
    redirect_host = None
    path_kind = 'none'
    if raw:
        from urllib.parse import urlsplit
        parsed = urlsplit(raw)
        redirect_host = parsed.hostname.lower() if parsed.hostname else None
        path_kind = 'handle' if re.fullmatch(r'/@[^/]+/?', parsed.path) else 'channel' if re.fullmatch(r'/channel/UC[A-Za-z0-9_-]{20,30}/?', parsed.path) else 'other'
    browse_id = browse.get('browseId', '')
    return {
        'endpointKind': 'browse' if browse else 'url' if url_endpoint else 'other',
        'redirectHost': redirect_host,
        'redirectPathKind': path_kind,
        'ucBrowseId': bool(re.fullmatch(r'UC[A-Za-z0-9_-]{20,30}', browse_id)),
    }

def post_resolve(request_host, api_key):
    client_name = 'WEB'
    payload = {
        'context': {'client': {'clientName': client_name, 'clientVersion': FIXED_WEB_VERSION, 'hl': 'en', 'gl': 'US'}},
        'url': PUBLIC_HANDLE_URL,
    }
    url = f'https://{request_host}/youtubei/v1/navigation/resolve_url?key={api_key}&prettyPrint=false'
    headers = {
        'Content-Type': 'application/json', 'Origin': f'https://{request_host}',
        'X-Origin': f'https://{request_host}', 'Referer': f'https://{request_host}/', 'User-Agent': UA,
    }
    request = urllib.request.Request(url, data=json.dumps(payload).encode('utf-8'), headers=headers, method='POST')
    with urllib.request.urlopen(request, timeout=10) as response:
        body = json.loads(response.read(512_000))
        return response.status, summarize(body)

results = []
for page_host in ['www.youtube.com', 'music.youtube.com']:
    try:
        key = public_key(page_host)
    except Exception as error:
        results.append({'pageConfigHost': page_host, 'requestHost': None, 'clientName': 'WEB', 'clientVersion': FIXED_WEB_VERSION, 'result': 'public_config_unavailable', 'reason': type(error).__name__})
        continue
    for request_host in ['www.youtube.com', 'music.youtube.com']:
        try:
            status, shape = post_resolve(request_host, key)
            results.append({'pageConfigHost': page_host, 'requestHost': request_host, 'clientName': 'WEB', 'clientVersion': FIXED_WEB_VERSION, 'httpStatus': status, **shape})
        except urllib.error.HTTPError as error:
            results.append({'pageConfigHost': page_host, 'requestHost': request_host, 'clientName': 'WEB', 'clientVersion': FIXED_WEB_VERSION, 'httpStatus': error.code, 'endpointKind': 'http_error'})
        except Exception as error:
            results.append({'pageConfigHost': page_host, 'requestHost': request_host, 'clientName': 'WEB', 'clientVersion': FIXED_WEB_VERSION, 'result': 'probe_unavailable', 'reason': type(error).__name__})

artifact = {
    'probedAtUtc': datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat().replace('+00:00', 'Z'),
    'source': 'unauthenticated public YouTube page config + InnerTube navigation/resolve_url',
    'client': {'name': 'WEB', 'version': FIXED_WEB_VERSION},
    'target': {'kind': 'fixed public channel handle', 'handleValueLogged': False, 'urlLogged': False},
    'credentials': {'cookiesSent': False, 'authorizationSent': False, 'apiKeyPersisted': False},
    'bounds': {'configReads': 2, 'resolveCalls': 4, 'maxPageBytes': 1000000, 'maxResponseBytes': 512000, 'timeoutSeconds': 10},
    'results': results,
}
raw = json.dumps(artifact, indent=2, ensure_ascii=False) + '\n'
path = '/tmp/youtube-channel-identity-patch/public-resolve-probe.json'
with open(path, 'w', encoding='utf-8') as file:
    file.write(raw)
print(raw, end='')
print('scriptSha256=' + hashlib.sha256(open(__file__, 'rb').read()).hexdigest())
