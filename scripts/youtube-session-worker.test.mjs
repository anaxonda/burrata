import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { executeYouTubeJob } from './youtube-session-worker.mjs';

const COOKIE = 'SAPISID=fixture-secret-cookie-value';
const ACCOUNT_ID = 'UCaaaaaaaaaaaaaaaaaaaaaa';
const SESSION = {
  origin: 'https://www.youtube.com', cookieHeader: COOKIE, accountId: ACCOUNT_ID,
  clientName: 'WEB', clientVersion: '1.20261001.00.00', apiKey: 'TEST-FIXTURE-CREDENTIAL-841FADDE18-AIzaFixtureKey0123456789',
  visitorData: 'fixtureVisitor', authUser: '0', hl: 'en', gl: 'US',
};
function accountResponse(items) {
  return {
    actions: [{
      getMultiPageMenuAction: {
        menu: {
          multiPageMenuRenderer: {
            sections: [{
              accountSectionListRenderer: {
                contents: [{ accountItemSectionRenderer: { contents: items.map(accountItem => ({ accountItem })) } }],
              },
            }],
          },
        },
      },
    }],
  };
}
const selectedAccount = accountResponse([
  { isSelected: true, accountName: { runs: [{ text: 'Fixture Channel' }] }, channelHandle: { runs: [{ text: '@fixture' }] } },
  { isSelected: false, accountName: { runs: [{ text: 'Other Channel' }] }, channelHandle: { runs: [{ text: '@other' }] } },
]);
const response = (body, status = 200, headers = {}) => new Response(typeof body === 'string' ? body : JSON.stringify(body), { status, headers });
const webAccountMenu = { actions: [{ openPopupAction: { popup: { multiPageMenuRenderer: { header: { activeAccountHeaderRenderer: { channelHandle: { simpleText: '@fixture' } } } } } } }] };


function fetchFixture({ accounts = selectedAccount, playlist = null } = {}) {
  const calls = [];
  const fetchImpl = async (url, options = {}) => {
    const parsed = new URL(url);
    calls.push({ url: parsed.href, method: options.method, headers: new Headers(options.headers), body: options.body ? JSON.parse(options.body) : null });
    if (parsed.pathname === '/youtubei/v1/account/account_menu') return response(webAccountMenu);
    if (parsed.pathname === '/youtubei/v1/account/accounts_list') return response(accounts);
    if (parsed.pathname === '/youtubei/v1/navigation/resolve_url') return response({ endpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } });
    if (parsed.pathname === '/youtubei/v1/browse' && playlist) return response(playlist);
    return response({ error: 'unexpected endpoint' }, 404);
  };
  return { fetchImpl, calls };
}

const job = (overrides = {}) => {
  const value = { prototypeEnabled: true, provider: 'youtube', operation: 'status', args: {}, session: SESSION, ...overrides };
  if (value.operation === 'applyPlaylistEdits' && value.args?.syncPreview === undefined) {
    value.args = { ...value.args, syncPreview: { previewId: 'fixture-preview-1', accountId: ACCOUNT_ID, baselineRevision: 'baseline-1', remoteRevision: 'remote-1', remoteHash: 'a'.repeat(64) } };
  }
  return value;
};

test('Music handoff browses general YouTube playlists with WWW authentication and bound continuations', async () => {
  const session = { ...SESSION, origin: 'https://music.youtube.com', clientName: 'WEB_REMIX', dataSyncId: 'fixture-sync' };
  const card = id => ({ gridPlaylistRenderer: { playlistId: id, title: { simpleText: 'Non-music fixture' } } });
  const fixture = fetchFixture();
  const fallbackFetch = fixture.fetchImpl;
  const libraryCalls = [];
  fixture.fetchImpl = async (url, options) => {
    const parsed = new URL(url);
    if (parsed.pathname !== '/youtubei/v1/browse') return fallbackFetch(url, options);
    const headers = new Headers(options.headers);
    libraryCalls.push({ parsed, options });
    assert.equal(parsed.origin, 'https://www.youtube.com');
    assert.equal(headers.get('origin'), parsed.origin);
    assert.equal(headers.get('x-youtube-client-name'), '1');
    assert.equal(headers.get('x-youtube-client-version'), '2.20260708.00.00');
    assert.equal(headers.get('cookie'), COOKIE);
    const timestamp = headers.get('authorization').match(/SAPISIDHASH (\d+)_/)[1];
    assert.equal(headers.get('authorization'), `SAPISIDHASH ${timestamp}_${createHash('sha1').update(`${timestamp} fixture-secret-cookie-value https://www.youtube.com`).digest('hex')}`);
    const body = JSON.parse(options.body);
    assert.equal(body.context.client.clientName, 'WEB');
    assert.equal(body.context.user.onBehalfOfUser, session.dataSyncId);
    assert.equal(headers.get('x-goog-pageid'), session.dataSyncId);
    if (!body.continuation) {
      assert.equal(body.browseId, 'FEplaylist_aggregation');
      return response({ contents: { twoColumnBrowseResultsRenderer: { tabs: [{ tabRenderer: { selected: true, content: { richGridRenderer: { contents: [card('PLfirst123'), { continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'general-next' } } } }] } } } }] } } });
    }
    assert.equal(body.continuation, 'general-next');
    return response({ onResponseReceivedActions: [{ appendContinuationItemsAction: { continuationItems: [card('PLsecond123')] } }] });
  };
  const first = await executeYouTubeJob(job({ operation: 'listPlaylists', session }), fixture);
  assert.equal(first.ok, true);
  assert.equal(first.result.items[0].id, 'PLfirst123');
  assert.match(first.result.nextCursor, /^yt-library-v1:/);
  const next = await executeYouTubeJob(job({ operation: 'listPlaylists', session, args: { cursor: first.result.nextCursor } }), fixture);
  assert.equal(next.ok, true);
  assert.equal(next.result.items[0].id, 'PLsecond123');
  assert.equal(next.result.nextCursor, null);
  assert.equal(libraryCalls.length, 2);
  for (const args of [{ cursor: 'raw-provider-token' }, { cursor: first.result.nextCursor, ownerId: 'UCbbbbbbbbbbbbbbbbbbbbbb' }, { cursor: `yt-library-v1:${encodeURIComponent(JSON.stringify({ accountId: 'UCbbbbbbbbbbbbbbbbbbbbbb', ownerId: '', origin: 'https://www.youtube.com', token: 'general-next' }))}` }]) {
    const rejected = await executeYouTubeJob(job({ operation: 'listPlaylists', session, args }), fixture);
    assert.equal(rejected.error.code, 'UNSUPPORTED');
  }
  assert.equal(libraryCalls.length, 2);
  const changed = await executeYouTubeJob(job({ operation: 'listPlaylists', session: { ...session, accountId: 'UCbbbbbbbbbbbbbbbbbbbbbb' }, args: { cursor: first.result.nextCursor } }), fixture);
  assert.equal(changed.error.code, 'ACCOUNT_CHANGED');
  assert.equal(libraryCalls.length, 2);
});

test('YouTube worker stays opt-in and rejects legacy/unpreviewed mutations before networking', async () => {
  const fixture = fetchFixture();
  assert.equal((await executeYouTubeJob({ ...job(), prototypeEnabled: false }, fixture)).error.code, 'UNSUPPORTED');
  assert.equal((await executeYouTubeJob(job({ operation: 'addTracks' }), fixture)).error.code, 'UNSUPPORTED');
  assert.equal(fixture.calls.length, 0);
});

test('YouTube playlist edit requires the exact complete reviewed snapshot and real occurrence ID', async () => {
  const playlist = {
    header: { musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLfixture123', ownerId: ACCOUNT_ID }, musicDetailHeaderRenderer: { secondSubtitle: { simpleText: '2 videos' } } },
    contents: { playlistVideoListRenderer: { contents: [
      { playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: 'entry1', title: { simpleText: 'Fixture song' } } },
      { playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: 'entry2', title: { simpleText: 'Duplicate song' } } },
    ] } },
  };
  const fixture = fetchFixture({ playlist });
  const expectedEntries = [{ entryId: 'entry1', trackId: 'abcdefghijk' }, { entryId: 'entry2', trackId: 'abcdefghijk' }];
  const base = { operation: 'applyPlaylistEdits', args: { previewApproved: true, playlistId: 'PLfixture123', expectedEntries, actions: [{ type: 'remove', entryId: 'entry2', trackId: 'abcdefghijk' }] } };
  const stale = await executeYouTubeJob(job({ ...base, args: { ...base.args, expectedEntries: [{ entryId: 'entry1', trackId: 'abcdefghijk' }] } }), fixture);
  assert.equal(stale.error.code, 'CONFLICT');
  assert.equal(fixture.calls.some(call => new URL(call.url).pathname.endsWith('/edit_playlist')), false);
  fixture.calls.length = 0;
  const duplicateOccurrence = await executeYouTubeJob(job({ ...base, args: { ...base.args, actions: [base.args.actions[0], base.args.actions[0]] } }), fixture);
  assert.equal(duplicateOccurrence.error.code, 'UNSUPPORTED');
  assert.equal(fixture.calls.some(call => new URL(call.url).pathname.endsWith('/edit_playlist')), false);
  fixture.calls.length = 0;
  fixture.fetchImpl = async (url, options = {}) => {
    const parsed = new URL(url);
    fixture.calls.push({ url: parsed.href, method: options.method, headers: new Headers(options.headers), body: options.body ? JSON.parse(options.body) : null });
    if (parsed.pathname === '/youtubei/v1/account/account_menu') return response(webAccountMenu);
    if (parsed.pathname === '/youtubei/v1/account/accounts_list') return response(selectedAccount);
    if (parsed.pathname === '/youtubei/v1/navigation/resolve_url') return response({ endpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } });
    if (parsed.pathname === '/youtubei/v1/browse/edit_playlist') return response({ status: 'STATUS_SUCCEEDED', playlistEditResults: [{ playlistEditVideoAddedResultData: { videoId: 'lmnopqrstuv', setVideoId: 'new-entry-1' } }] });
    if (parsed.pathname === '/youtubei/v1/browse') return response(playlist);
    return response({}, 404);
  };
  const applied = await executeYouTubeJob(job(base), fixture);
  assert.equal(applied.ok, true);
  assert.equal(applied.result.applied, 1);
  const edit = fixture.calls.find(call => new URL(call.url).pathname.endsWith('/edit_playlist'));
  assert.deepEqual(edit.body.actions, [{ action: 'ACTION_REMOVE_VIDEO', setVideoId: 'entry2', removedVideoId: 'abcdefghijk' }]);
});

test('YouTube playlist edit refuses to run without explicit preview approval', async () => {
  const fixture = fetchFixture();
  const result = await executeYouTubeJob(job({ operation: 'applyPlaylistEdits', args: { playlistId: 'PLfixture123', expectedEntries: [], actions: [{ type: 'add', trackId: 'abcdefghijk' }] } }), fixture);
  assert.equal(result.error.code, 'UNSUPPORTED');
  assert.equal(fixture.calls.some(call => new URL(call.url).pathname.endsWith('/edit_playlist')), false);
});

test('YouTube system collections LM and SE can be read but never edited', async () => {
  for (const id of ['LM', 'SE']) {
    const fixture = fetchFixture({ playlist: {
      header: { musicEditablePlaylistDetailHeaderRenderer: { playlistId: id, ownerId: ACCOUNT_ID }, musicDetailHeaderRenderer: { secondSubtitle: { simpleText: '1 video' } } },
      contents: { playlistVideoListRenderer: { contents: [{ playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: 'fixture-entry' } }] } },
    } });
    const read = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: id } }), fixture);
    assert.equal(read.ok, true);
    assert.equal(read.result.playlist.id, id);
    assert.equal(read.result.playlist.canEdit, false);
    assert.equal(read.result.playlist.title, id === 'LM' ? 'Liked Music' : 'Saved Episodes');
    assert.equal(read.result.entries.length, 1);
    const browse = fixture.calls.find(call => new URL(call.url).pathname.endsWith('/browse'));
    assert.equal(browse.body.browseId, `VL${id}`);
    fixture.calls.length = 0;
    const write = await executeYouTubeJob(job({ operation: 'applyPlaylistEdits', args: {
      playlistId: id, previewApproved: true, expectedEntries: [{ entryId: 'fixture-entry', trackId: 'abcdefghijk' }],
      actions: [{ type: 'remove', entryId: 'fixture-entry', trackId: 'abcdefghijk' }],
    } }), fixture);
    assert.equal(write.error.code, 'UNSUPPORTED');
    assert.equal(fixture.calls.some(call => new URL(call.url).pathname.includes('/browse')), false);
  }
  const fixture = fetchFixture();
  const invalid = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'XX' } }), fixture);
  assert.equal(invalid.error.code, 'UNSUPPORTED');
  assert.equal(fixture.calls.some(call => new URL(call.url).pathname.includes('/browse')), false);
});

test('YouTube playlist edits bind the stored preview to the independently verified account', async () => {
  const playlist = {
    header: { musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLfixture123', ownerId: ACCOUNT_ID }, musicDetailHeaderRenderer: { secondSubtitle: { simpleText: '1 video' } } },
    contents: { playlistVideoListRenderer: { contents: [{ playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: 'entry1' } }] } },
  };
  const fixture = fetchFixture({ playlist });
  const result = await executeYouTubeJob(job({
    operation: 'applyPlaylistEdits',
    args: {
      previewApproved: true,
      playlistId: 'PLfixture123',
      expectedEntries: [{ entryId: 'entry1', trackId: 'abcdefghijk' }],
      actions: [{ type: 'remove', entryId: 'entry1', trackId: 'abcdefghijk' }],
      syncPreview: { previewId: 'fixture-preview-1', accountId: 'different-channel', baselineRevision: 'baseline-1', remoteRevision: 'remote-1', remoteHash: 'a'.repeat(64) },
    },
  }), fixture);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'CONFLICT');
  assert.equal(fixture.calls.some(call => new URL(call.url).pathname.endsWith('/edit_playlist')), false);
});

test('YouTube playlist edit rejects an empty-looking snapshot with unknown count or wrong editable playlist ID', async () => {
  for (const [header, errorCode] of [
    [{ musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLfixture123', ownerId: ACCOUNT_ID } }, 'PROVIDER_CHANGED'],
    [{ musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLanother123', ownerId: ACCOUNT_ID }, musicDetailHeaderRenderer: { secondSubtitle: { simpleText: '0 videos' } } }, 'UNSUPPORTED'],
  ]) {
    const fixture = fetchFixture({ playlist: { header, contents: { playlistVideoListRenderer: { contents: [] } } } });
    const result = await executeYouTubeJob(job({ operation: 'applyPlaylistEdits', args: { previewApproved: true, playlistId: 'PLfixture123', expectedEntries: [], actions: [{ type: 'add', trackId: 'abcdefghijk' }] } }), fixture);
    assert.equal(result.ok, false);
    assert.equal(result.error.code, errorCode);
    assert.equal(fixture.calls.some(call => new URL(call.url).pathname.endsWith('/edit_playlist')), false);
  }
});

test('YouTube playlist edit fails closed on an unfamiliar mutation response', async () => {
  const playlist = {
    header: { musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLfixture123', ownerId: ACCOUNT_ID }, musicDetailHeaderRenderer: { secondSubtitle: { simpleText: '1 video' } } },
    contents: { playlistVideoListRenderer: { contents: [{ playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: 'entry1' } }] } },
  };
  const fixture = fetchFixture({ playlist });
  const originalFetch = fixture.fetchImpl;
  fixture.fetchImpl = async (url, options = {}) => {
    if (new URL(url).pathname.endsWith('/edit_playlist')) return response({ status: 'STATUS_CONTINUATION' });
    return originalFetch(url, options);
  };
  const result = await executeYouTubeJob(job({ operation: 'applyPlaylistEdits', args: { previewApproved: true, playlistId: 'PLfixture123', expectedEntries: [{ entryId: 'entry1', trackId: 'abcdefghijk' }], actions: [{ type: 'add', trackId: 'lmnopqrstuv' }] } }), fixture);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'OUTCOME_UNKNOWN');
  assert.equal(JSON.stringify(result).includes(COOKIE), false);
});

test('YouTube playlist edit reports unknown outcome after a dispatched request times out', async () => {
  const playlist = {
    header: { musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLfixture123', ownerId: ACCOUNT_ID }, musicDetailHeaderRenderer: { secondSubtitle: { simpleText: '1 video' } } },
    contents: { playlistVideoListRenderer: { contents: [{ playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: 'entry1' } }] } },
  };
  const fixture = fetchFixture({ playlist });
  const original = fixture.fetchImpl;
  fixture.fetchImpl = async (url, options = {}) => {
    if (new URL(url).pathname.endsWith('/edit_playlist')) throw new Error('connection reset');
    return original(url, options);
  };
  const result = await executeYouTubeJob(job({ operation: 'applyPlaylistEdits', args: { previewApproved: true, playlistId: 'PLfixture123', expectedEntries: [{ entryId: 'entry1', trackId: 'abcdefghijk' }], actions: [{ type: 'add', trackId: 'lmnopqrstuv' }] } }), fixture);
  assert.equal(result.error.code, 'OUTCOME_UNKNOWN');
  assert.match(result.error.message, /Refresh the playlist/);
});

test('YouTube playlist edit marks mismatched, invalid, or duplicate add receipts as unknown outcome', async () => {
  const playlist = {
    header: { musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLfixture123', ownerId: ACCOUNT_ID }, musicDetailHeaderRenderer: { secondSubtitle: { simpleText: '1 video' } } },
    contents: { playlistVideoListRenderer: { contents: [{ playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: 'entry1' } }] } },
  };
  const cases = [
    [{ playlistEditVideoAddedResultData: { videoId: 'wrongwrong1', setVideoId: 'new-entry' } }],
    [{ playlistEditVideoAddedResultData: { videoId: 'lmnopqrstuv', setVideoId: 'bad occurrence id' } }],
    [
      { playlistEditVideoAddedResultData: { videoId: 'lmnopqrstuv', setVideoId: 'same-entry' } },
      { playlistEditVideoAddedResultData: { videoId: 'mnopqrstuvw', setVideoId: 'same-entry' } },
    ],
  ];
  for (const playlistEditResults of cases) {
    const fixture = fetchFixture({ playlist });
    const original = fixture.fetchImpl;
    fixture.fetchImpl = async (url, options = {}) => new URL(url).pathname.endsWith('/edit_playlist')
      ? response({ status: 'STATUS_SUCCEEDED', playlistEditResults })
      : original(url, options);
    const actions = [{ type: 'add', trackId: 'lmnopqrstuv' }];
    if (playlistEditResults.length === 2) actions.push({ type: 'add', trackId: 'mnopqrstuvw' });
    const result = await executeYouTubeJob(job({ operation: 'applyPlaylistEdits', args: { previewApproved: true, playlistId: 'PLfixture123', expectedEntries: [{ entryId: 'entry1', trackId: 'abcdefghijk' }], actions } }), fixture);
    assert.equal(result.error.code, 'OUTCOME_UNKNOWN');
    assert.match(result.error.message, /Refresh the playlist/);
  }
});

test('YouTube status independently verifies selected identity and never exposes the session secret', async () => {
  const fixture = fetchFixture();
  const result = await executeYouTubeJob(job(), { ...fixture, now: () => 1_800_000_000_000 });
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.deepEqual(JSON.parse(JSON.stringify(result.result)), { accountId: ACCOUNT_ID, displayName: 'Fixture Channel' });
  assert.deepEqual(fixture.calls.map(call => new URL(call.url).pathname), ['/youtubei/v1/account/accounts_list', '/youtubei/v1/navigation/resolve_url']);
  for (const call of fixture.calls) {
    assert.equal(new URL(call.url).origin, SESSION.origin);
    assert.equal(call.headers.get('cookie'), COOKIE);
    assert.match(call.headers.get('authorization'), /^SAPISIDHASH \d+_[a-f0-9]{40}$/);
    assert.equal(call.headers.has('proxy-authorization'), false);
  }
  assert.equal(JSON.stringify(result).includes(COOKIE), false);
});

test('native bootstrap extracts static YouTube client config then verifies the selected channel', async () => {
  const calls = [];
  const config = {
    INNERTUBE_CLIENT_NAME: 'WEB', INNERTUBE_CLIENT_VERSION: '1.20261001.00.00',
    INNERTUBE_API_KEY: 'TEST-FIXTURE-CREDENTIAL-841FADDE18-AIzaFixtureKey0123456789', VISITOR_DATA: 'fixtureVisitor',
    DATASYNC_ID: 'V1:datasync-fixture||opaque-suffix', SESSION_INDEX: '0', HL: 'en', GL: 'US',
  };
  const fetchImpl = async (url, options = {}) => {
    const parsed = new URL(url);
    calls.push({ url: parsed.href, method: options.method, headers: new Headers(options.headers), body: options.body ? JSON.parse(options.body) : null });
    if (parsed.pathname === '/') return response(`<html><script>ytcfg.set(${JSON.stringify(config)});</script></html>`);
    if (parsed.pathname === '/youtubei/v1/account/account_menu') return response(webAccountMenu);
    if (parsed.pathname === '/youtubei/v1/account/accounts_list') return response(selectedAccount);
    if (parsed.pathname === '/youtubei/v1/navigation/resolve_url') return response({ endpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } });
    return response({}, 404);
  };
  const result = await executeYouTubeJob(job({
    operation: 'bootstrap', session: { origin: SESSION.origin, cookieHeader: COOKIE, accountId: '@pending', displayName: 'YouTube account' },
  }), { fetchImpl, now: () => 1_800_000_000_000 });
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.accountId, ACCOUNT_ID);
  assert.equal(result.result.displayName, 'Fixture Channel');
  assert.equal(result.result.clientVersion, config.INNERTUBE_CLIENT_VERSION);
  assert.equal(result.result.apiKey, config.INNERTUBE_API_KEY);
  assert.equal(result.result.dataSyncId, 'V1:datasync-fixture');
  assert.deepEqual(calls.map(call => new URL(call.url).pathname), ['/', '/youtubei/v1/account/accounts_list', '/youtubei/v1/navigation/resolve_url']);
  assert.equal(calls[0].headers.get('cookie'), COOKIE);
  assert.equal(new URL(calls[1].url).searchParams.get('key'), config.INNERTUBE_API_KEY);
  assert.equal(calls[1].headers.get('x-youtube-client-version'), '2.20260708.00.00');
  assert.equal(calls[1].body.context.user.onBehalfOfUser, undefined, 'accounts_list must omit per-account context');
  assert.equal(JSON.stringify(result).includes(COOKIE), false);
});

test('native bootstrap uses validated dataSyncId for account_menu and rejects a mismatched supplied context', async () => {
  const calls = [];
  const config = { INNERTUBE_CLIENT_NAME: 'WEB', INNERTUBE_CLIENT_VERSION: '1.20261001.00.00', DATASYNC_ID: 'V1:account-context||opaque', SESSION_INDEX: '0' };
  const fetchImpl = async (url, options = {}) => {
    const parsed = new URL(url);
    calls.push({ path: parsed.pathname, body: options.body ? JSON.parse(options.body) : null });
    if (parsed.pathname === '/') return response(`<script>ytcfg.set(${JSON.stringify(config)});</script>`);
    if (parsed.pathname.endsWith('/accounts_list')) return response({ responseContext: {} });
    if (parsed.pathname.endsWith('/account_menu')) return response({ actions: [{ openPopupAction: { popup: { multiPageMenuRenderer: { header: { activeAccountHeaderRenderer: {
      accountName: { simpleText: 'Fixture Channel' }, channelHandle: { simpleText: '@fixture' },
    } } } } } }] });
    if (parsed.pathname.endsWith('/resolve_url')) return response({ endpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } });
    return response({}, 404);
  };
  const result = await executeYouTubeJob(job({ operation: 'bootstrap', session: { origin: SESSION.origin, cookieHeader: COOKIE } }), { fetchImpl });
  assert.equal(result.ok, true, JSON.stringify(result));
  const list = calls.find(call => call.path.endsWith('/accounts_list'));
  const menu = calls.find(call => call.path.endsWith('/account_menu'));
  assert.equal(list.body.context.user.onBehalfOfUser, undefined);
  assert.equal(menu.body.context.user.onBehalfOfUser, 'V1:account-context');
  assert.equal(result.result.dataSyncId, 'V1:account-context');

  const mismatch = await executeYouTubeJob(job({ operation: 'bootstrap', session: { origin: SESSION.origin, cookieHeader: COOKIE, dataSyncId: 'V1:other-context' } }), { fetchImpl });
  assert.equal(mismatch.ok, false);
  assert.equal(mismatch.error.code, 'ACCOUNT_CHANGED');
  assert.equal(mismatch.error.phase, 'account_identity');
  assert.equal(JSON.stringify(mismatch).includes('V1:'), false);
});

test('native bootstrap resolves a Unicode handle from the active Music header and reports only structural identity diagnostics', async () => {
  const calls = [];
  const greekHandle = 'δοκιμή';
  const config = { INNERTUBE_CLIENT_NAME: 'WEB_REMIX', INNERTUBE_CLIENT_VERSION: '1.20261001.00.00', INNERTUBE_API_KEY: 'TEST-FIXTURE-CREDENTIAL-841FADDE18-AIzaFixtureKey0123456789', VISITOR_DATA: 'fixtureVisitor', SESSION_INDEX: '0', LOGGED_IN: true };
  const accounts = accountResponse([{ isSelected: true, accountName: { runs: [{ text: 'Private fixture account name' }] }, serviceEndpoint: { selectActiveIdentityEndpoint: { supportedTokens: [{ pageIdToken: { pageId: '123456789012345' } }] } } }]);
  const fetchImpl = async (url, options = {}) => {
    const parsed = new URL(url), body = options.body ? JSON.parse(options.body) : null;
    calls.push({ url: parsed.href, path: parsed.pathname, headers: new Headers(options.headers), body });
    if (parsed.pathname === '/') return response(`<script>ytcfg.set(${JSON.stringify(config)});</script>`);
    if (parsed.pathname.endsWith('/accounts_list')) return response(accounts);
    if (parsed.pathname.endsWith('/account_menu')) {
      const activeAccountHeaderRenderer = { accountName: { runs: [{ text: 'Private fixture account name' }] }, channelHandle: { runs: [{ text: `@${greekHandle}` }] } };
      return response({ actions: [{ openPopupAction: { popup: { multiPageMenuRenderer: { header: { activeAccountHeaderRenderer } } } } }] });
    }
    if (parsed.pathname.endsWith('/resolve_url') && body.url === `https://www.youtube.com/@${encodeURIComponent(greekHandle)}`) {
      if (calls.filter(call => call.path.endsWith('/resolve_url')).length === 1) return response({ endpoint: { urlEndpoint: { url: `https://www.youtube.com/@${encodeURIComponent(greekHandle)}?feature=accounts` } } });
      if (calls.filter(call => call.path.endsWith('/resolve_url')).length === 2) return response({ endpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } });
    }
    return response({}, 404);
  };
  const result = await executeYouTubeJob(job({ operation: 'bootstrap', session: { origin: 'https://music.youtube.com', cookieHeader: COOKIE, accountId: '@pending' } }), { fetchImpl, now: () => 1_800_000_000_000 });
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.accountId, ACCOUNT_ID);
  const resolveCalls = calls.filter(call => call.path.endsWith('/resolve_url'));
  assert.equal(resolveCalls.length, 2);
  assert.equal(new URL(resolveCalls[0].url).origin, 'https://www.youtube.com');
  assert.equal(resolveCalls[0].body.context.client.clientName, 'WEB');
  assert.equal(resolveCalls[0].headers.get('x-youtube-client-name'), '1');
  assert.equal(resolveCalls[0].body.context.user.onBehalfOfUser, undefined);
  assert.equal(resolveCalls[0].body.url, `https://www.youtube.com/@${encodeURIComponent(greekHandle)}`);
  assert.equal(resolveCalls[1].body.url, `https://www.youtube.com/@${encodeURIComponent(greekHandle)}`);
  assert.equal(JSON.stringify(result).includes(COOKIE), false);
});

test('native bootstrap distinguishes authenticated missing-channel identity and emits allowlisted booleans only', async () => {
  const privateName = 'Private fixture account name';
  const config = { INNERTUBE_CLIENT_NAME: 'WEB_REMIX', INNERTUBE_CLIENT_VERSION: '1.20261001.00.00', INNERTUBE_API_KEY: 'TEST-FIXTURE-CREDENTIAL-841FADDE18-AIzaFixtureKey0123456789', VISITOR_DATA: 'fixtureVisitor', SESSION_INDEX: '0', LOGGED_IN: true };
  const accounts = accountResponse([{ isSelected: true, accountName: { runs: [{ text: privateName }] }, serviceEndpoint: { selectActiveIdentityEndpoint: { supportedTokens: [{ pageIdToken: { pageId: '123456789012345' } }] } } }]);
  const fetchImpl = async (url, options = {}) => {
    const parsed = new URL(url);
    if (parsed.pathname === '/') return response(`<script>ytcfg.set(${JSON.stringify(config)});</script>`);
    if (parsed.pathname.endsWith('/accounts_list')) return response(accounts);
    if (parsed.pathname.endsWith('/account_menu')) {
      const activeAccountHeaderRenderer = { accountName: { runs: [{ text: privateName }] } };
      return response({ actions: [{ openPopupAction: { popup: { multiPageMenuRenderer: { header: { activeAccountHeaderRenderer } } } } }] });
    }
    return response({}, 404);
  };
  const result = await executeYouTubeJob(job({ operation: 'bootstrap', session: { origin: 'https://music.youtube.com', cookieHeader: COOKIE, accountId: '@pending' } }), { fetchImpl, now: () => 1_800_000_000_000 });
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'PROVIDER_CHANGED');
  assert.equal(result.error.phase, 'channel_identity');
  assert.match(result.error.message, /authenticated account, but Burrata could not verify its active channel ID/);
  assert.match(result.error.message, /activeHeader=true headerHandle=false selectedHandle=false pageIdToken=true/);
  assert.equal(JSON.stringify(result).includes(privateName), false);
  assert.equal(JSON.stringify(result).includes(COOKIE), false);
});

test('captured page context survives incomplete HTTP config and permits visitorData rotation', async () => {
  const calls = [];
  const config = { INNERTUBE_CLIENT_NAME: 'WEB', INNERTUBE_CLIENT_VERSION: '1.20261001.00.00', VISITOR_DATA: 'rotated-visitor' };
  const fetchImpl = async (url, options = {}) => {
    const parsed = new URL(url);
    calls.push({ path: parsed.pathname, headers: new Headers(options.headers), body: options.body ? JSON.parse(options.body) : null });
    if (parsed.pathname === '/') return response(`<script>ytcfg.set(${JSON.stringify(config)});</script>`);
    if (parsed.pathname.endsWith('/accounts_list')) return response(selectedAccount);
    if (parsed.pathname.endsWith('/resolve_url')) return response({ endpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } });
    return response({}, 404);
  };
  const session = { origin: SESSION.origin, cookieHeader: COOKIE, accountId: '@pending', displayName: 'YouTube account', visitorData: 'page-visitor', dataSyncId: 'V1:page-sync', authUser: '2', pageContextCaptured: true };
  const result = await executeYouTubeJob(job({ operation: 'bootstrap', session }), { fetchImpl });
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.visitorData, 'rotated-visitor');
  assert.equal(result.result.dataSyncId, 'V1:page-sync');
  assert.equal(result.result.authUser, '2');
  const accounts = calls.find(call => call.path.endsWith('/accounts_list'));
  assert.equal(accounts.headers.get('x-goog-authuser'), '2');
  assert.equal(accounts.body.context.client.visitorData, 'rotated-visitor');
  assert.equal(accounts.body.context.user.onBehalfOfUser, undefined);
});

test('captured page SESSION_INDEX mismatch rejects bootstrap without returning context', async () => {
  const config = { INNERTUBE_CLIENT_NAME: 'WEB', INNERTUBE_CLIENT_VERSION: '1.20261001.00.00', SESSION_INDEX: '1' };
  const result = await executeYouTubeJob(job({
    operation: 'bootstrap',
    session: { origin: SESSION.origin, cookieHeader: COOKIE, accountId: '@pending', visitorData: 'page-visitor', authUser: '2', pageContextCaptured: true },
  }), { fetchImpl: async () => response(`<script>ytcfg.set(${JSON.stringify(config)});</script>`) });
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'ACCOUNT_CHANGED');
  assert.equal(result.error.phase, 'account_identity');
  assert.equal(JSON.stringify(result).includes('page-visitor'), false);
});

test('captured page context is retained when fetched ytcfg omits visitor, sync ID, and session index', async () => {
  const calls = [];
  const config = { INNERTUBE_CLIENT_NAME: 'WEB', INNERTUBE_CLIENT_VERSION: '1.20261001.00.00' };
  const fetchImpl = async (url, options = {}) => {
    const parsed = new URL(url);
    calls.push({ path: parsed.pathname, headers: new Headers(options.headers), body: options.body ? JSON.parse(options.body) : null });
    if (parsed.pathname === '/') return response(`<script>ytcfg.set(${JSON.stringify(config)});</script>`);
    if (parsed.pathname.endsWith('/accounts_list')) return response(selectedAccount);
    if (parsed.pathname.endsWith('/resolve_url')) return response({ endpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } });
    return response({}, 404);
  };
  const session = { origin: SESSION.origin, cookieHeader: COOKIE, accountId: '@pending', visitorData: 'page-visitor', dataSyncId: 'V1:page-sync', authUser: '2', pageContextCaptured: true };
  const result = await executeYouTubeJob(job({ operation: 'bootstrap', session }), { fetchImpl });
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.visitorData, 'page-visitor');
  assert.equal(result.result.dataSyncId, 'V1:page-sync');
  assert.equal(result.result.authUser, '2');
  const accounts = calls.find(call => call.path.endsWith('/accounts_list'));
  assert.equal(accounts.headers.get('x-goog-authuser'), '2');
  assert.equal(accounts.body.context.client.visitorData, 'page-visitor');
});

test('worker rejects malformed persisted dataSyncId before making a request', async () => {
  const fixture = fetchFixture();
  const result = await executeYouTubeJob(job({ session: { ...SESSION, dataSyncId: 'invalid context!' } }), fixture);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'UNSUPPORTED');
  assert.equal(fixture.calls.length, 0);
});

test('native bootstrap forwards the logged-in header only for strict ytcfg true', async () => {
  for (const loggedIn of [true, false, 'unknown']) {
    const calls = [];
    const config = { INNERTUBE_CLIENT_NAME: 'WEB', INNERTUBE_CLIENT_VERSION: '1.20261001.00.00', SESSION_INDEX: '0', ...(loggedIn === 'unknown' ? {} : { LOGGED_IN: loggedIn }) };
    const fetchImpl = async (url, options = {}) => {
      const parsed = new URL(url);
      calls.push({ path: parsed.pathname, headers: new Headers(options.headers) });
      if (parsed.pathname === '/') return response(`<script>ytcfg.set(${JSON.stringify(config)});</script>`);
      if (parsed.pathname.endsWith('/accounts_list')) return response(selectedAccount);
      if (parsed.pathname.endsWith('/resolve_url')) return response({ endpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } });
      return response({}, 404);
    };
    const result = await executeYouTubeJob(job({ operation: 'bootstrap', session: { origin: SESSION.origin, cookieHeader: COOKIE } }), { fetchImpl });
    assert.equal(result.ok, true, JSON.stringify(result));
    const accountCall = calls.find(call => call.path.endsWith('/accounts_list'));
    assert.equal(accountCall.headers.get('x-youtube-bootstrap-logged-in'), loggedIn === true ? 'true' : null);
  }
});

test('account-selection diagnostics include only the fixed bootstrap logged-in tri-state', async () => {
  const config = { INNERTUBE_CLIENT_NAME: 'WEB', INNERTUBE_CLIENT_VERSION: '1.20261001.00.00', SESSION_INDEX: '0', LOGGED_IN: true };
  const fetchImpl = async url => {
    const path = new URL(url).pathname;
    if (path === '/') return response(`<script>ytcfg.set(${JSON.stringify(config)});</script>`);
    if (path.endsWith('/accounts_list')) return response({ responseContext: {} });
    if (path.endsWith('/account_menu')) return response({ compactLinks: [{ compactLinkRenderer: {} }] });
    return response({}, 404);
  };
  const result = await executeYouTubeJob(job({ operation: 'bootstrap', session: { origin: SESSION.origin, cookieHeader: COOKIE } }), { fetchImpl });
  assert.equal(result.ok, false);
  assert.equal(result.error.phase, 'account_selection');
  assert.match(result.error.message, /bootstrapLoggedIn=true/);
  assert.equal(JSON.stringify(result).includes(JSON.stringify(config)), false);
});

test('native bootstrap uses Meld-compatible WEB discovery on WWW and keeps later calls on Music', async () => {
  const calls = [];
  const config = { INNERTUBE_CLIENT_NAME: 67, INNERTUBE_CLIENT_VERSION: '1.20261001.00.00', INNERTUBE_API_KEY: 'TEST-FIXTURE-CREDENTIAL-841FADDE18-AIzaFixtureKey0123456789', VISITOR_DATA: 'fixtureVisitor', SESSION_INDEX: 0, HL: 'en', GL: 'US' };
  const fetchImpl = async (url, options = {}) => {
    const parsed = new URL(url);
    calls.push({ origin: parsed.origin, path: parsed.pathname, headers: new Headers(options.headers), body: options.body ? JSON.parse(options.body) : null });
    if (parsed.pathname === '/') {
      if (!/Mozilla\/5\.0.*Chrome\//.test(new Headers(options.headers).get('user-agent') || '')) return response('<title>Your browser is deprecated. Please upgrade.</title>');
      return response(`<script>ytcfg.set(${JSON.stringify(config)});</script>`);
    }
    if (parsed.pathname === '/youtubei/v1/account/account_menu') return response(webAccountMenu);
    if (parsed.pathname === '/youtubei/v1/account/accounts_list') return response(selectedAccount);
    if (parsed.pathname === '/youtubei/v1/navigation/resolve_url') return response({ endpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } });
    return response({}, 404);
  };
  const musicOrigin = 'https://music.youtube.com';
  const result = await executeYouTubeJob(job({ operation: 'bootstrap', session: { origin: musicOrigin, cookieHeader: COOKIE, accountId: '@pending' } }), { fetchImpl, now: () => 1_800_000_000_000 });
  assert.equal(result.ok, true, JSON.stringify(result));
  const accountCall = calls.find(call => call.path.endsWith('/accounts_list'));
  assert.equal(accountCall.origin, 'https://www.youtube.com');
  assert.equal(accountCall.headers.get('origin'), 'https://www.youtube.com');
  assert.equal(accountCall.headers.get('referer'), 'https://www.youtube.com/');
  assert.equal(accountCall.headers.get('x-youtube-client-name'), '1');
  assert.equal(accountCall.headers.get('x-youtube-client-version'), '2.20260708.00.00');
  assert.equal(accountCall.body.context.client.clientName, 'WEB');
  assert.equal(accountCall.body.context.client.clientVersion, '2.20260708.00.00');
  const auth = accountCall.headers.get('authorization');
  const timestamp = /^SAPISIDHASH (\d{1,12})_/.exec(auth)?.[1];
  assert.ok(timestamp);
  assert.equal(auth, `SAPISIDHASH ${timestamp}_${createHash('sha1').update(`${timestamp} fixture-secret-cookie-value https://www.youtube.com`).digest('hex')}`);
  assert.equal(accountCall.body.requestType, 'ACCOUNTS_LIST_REQUEST_TYPE_CHANNEL_SWITCHER');
  assert.equal(accountCall.body.callCircumstance, 'SWITCHING_USERS_FULL');
  const resolveCall = calls.find(call => call.path.endsWith('/navigation/resolve_url'));
  assert.equal(resolveCall.origin, 'https://www.youtube.com');
  assert.equal(resolveCall.headers.get('origin'), 'https://www.youtube.com');
  assert.equal(resolveCall.headers.get('x-youtube-client-name'), '1');
  assert.equal(resolveCall.headers.get('x-youtube-client-version'), '2.20260708.00.00');
  assert.equal(resolveCall.body.context.client.clientName, 'WEB');
  assert.equal(resolveCall.body.context.user.onBehalfOfUser, undefined);
  assert.equal(result.result.clientName, 'WEB_REMIX');
});

test('native bootstrap rejects unknown client IDs and preserves safe server-decline diagnostics', async () => {
  for (const clientName of [999, 'UNKNOWN_CLIENT']) {
    const calls = [];
    const rejectedConfig = { INNERTUBE_CLIENT_NAME: clientName, INNERTUBE_CLIENT_VERSION: '1.20261001.00.00' };
    const rejected = await executeYouTubeJob(job({ operation: 'bootstrap', session: { origin: SESSION.origin, cookieHeader: COOKIE } }), {
      fetchImpl: async url => { calls.push(new URL(url).pathname); return response(`<script>ytcfg.set(${JSON.stringify(rejectedConfig)});</script>`); },
    });
    assert.equal(rejected.error.code, 'PROVIDER_CHANGED');
    assert.equal(rejected.error.phase, 'bootstrap_config');
    assert.deepEqual(calls, ['/']);
  }
  const declined = await executeYouTubeJob(job({ operation: 'bootstrap', session: { origin: SESSION.origin, cookieHeader: COOKIE } }), {
    fetchImpl: async () => response('declined', 400),
  });
  assert.equal(declined.error.code, 'PROVIDER_CHANGED');
  assert.equal(declined.error.phase, 'bootstrap');
  assert.equal(declined.error.httpStatus, 400);
  assert.match(declined.error.message, /phase=bootstrap, HTTP 400/);
  assert.equal(JSON.stringify(declined).includes(COOKIE), false);
});

test('native bootstrap rejects malformed or oversized DATASYNC_ID config without exposing it', async () => {
  for (const dataSyncId of [7, {}, `V1:${'x'.repeat(600)}`, '||suffix']) {
    const config = { INNERTUBE_CLIENT_NAME: 'WEB', INNERTUBE_CLIENT_VERSION: '1.20261001.00.00', DATASYNC_ID: dataSyncId, SESSION_INDEX: '0' };
    const result = await executeYouTubeJob(job({ operation: 'bootstrap', session: { origin: SESSION.origin, cookieHeader: COOKIE } }), {
      fetchImpl: async () => response(`<script>ytcfg.set(${JSON.stringify(config)});</script>`),
    });
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'PROVIDER_CHANGED');
    assert.equal(result.error.phase, 'bootstrap_config');
    assert.equal(JSON.stringify(result).includes(JSON.stringify(dataSyncId)), false);
  }
});

test('YouTube worker rejects a switched active identity and does not issue the requested browse', async () => {
  const fixture = fetchFixture({ accounts: accountResponse([{ isSelected: true, accountName: { runs: [{ text: 'Fixture Channel' }] }, channelHandle: { runs: [{ text: '@fixture' }] } }]) });
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' }, session: { ...SESSION, accountId: 'UCbbbbbbbbbbbbbbbbbbbbbb' } }), fixture);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'ACCOUNT_CHANGED');
  assert.equal(result.error.phase, 'account_identity');
  assert.deepEqual(fixture.calls.map(call => new URL(call.url).pathname), ['/youtubei/v1/account/accounts_list', '/youtubei/v1/navigation/resolve_url']);
});

test('YouTube handoff handle is compared to the authenticated selected channel and returns its server ID', async () => {
  const fixture = fetchFixture({ accounts: accountResponse([{ isSelected: true, accountName: { runs: [{ text: 'Fixture Channel' }] }, channelHandle: { runs: [{ text: '@fixture' }] } }]) });
  const result = await executeYouTubeJob(job({ session: { ...SESSION, accountId: '@fixture' } }), fixture);
  assert.equal(result.ok, true);
  assert.equal(result.result.accountId, ACCOUNT_ID);
  assert.equal(result.result.displayName, 'Fixture Channel');
});

test('YouTube worker refuses identity responses without exactly one selected account', async () => {
  const fixture = fetchFixture({ accounts: accountResponse([{ isSelected: false, accountName: { runs: [{ text: 'Fixture Channel' }] }, channelHandle: { runs: [{ text: '@fixture' }] } }]) });
  const result = await executeYouTubeJob(job(), fixture);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'SIGN_IN_REQUIRED');
  assert.equal(result.error.phase, 'account_selection');
  assert.match(result.error.message, /items=1, selected=0, renderers=0/);
  assert.equal(fixture.calls.length, 1);
});

test('YouTube authentication rejection gives an explicit same-channel reconnect action', async () => {
  const fixture = fetchFixture({ accounts: null });
  fixture.fetchImpl = async (url, options = {}) => {
    fixture.calls.push({ url: new URL(url).href, method: options.method, headers: new Headers(options.headers), body: options.body ? JSON.parse(options.body) : null });
    return response({ error: 'private provider details' }, 401);
  };
  const result = await executeYouTubeJob(job(), fixture);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'SIGN_IN_REQUIRED');
  assert.match(result.error.message, /Sign in to YouTube again and confirm your channel/);
  assert.doesNotMatch(result.error.message, /private provider details/);
});

test('YouTube getPlaylist keeps normalized entries and the provider continuation', async () => {
  const playlist = {
    header: { playlistHeaderRenderer: { title: { simpleText: 'Fixture list' }, numVideosText: { runs: [{ text: '2' }] } } },
    contents: {
      twoColumnBrowseResultsRenderer: {
        tabs: [{ tabRenderer: { content: { sectionListRenderer: { contents: [
          { playlistVideoListRenderer: { contents: [{ playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: 'entry1', title: { simpleText: 'Fixture song' }, shortBylineText: { simpleText: 'Fixture Artist' }, lengthText: { simpleText: '3:14' } } }] } },
          { continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'next-page' } } } },
        ] } } } }],
      },
    },
  };
  const brandPageId = '123456789012345678901';
  const accounts = accountResponse([{ isSelected: true, accountName: { runs: [{ text: 'Fixture Channel' }] }, channelHandle: { runs: [{ text: '@fixture' }] }, serviceEndpoint: { selectActiveIdentityEndpoint: { supportedTokens: [{ pageIdToken: { pageId: brandPageId } }] } } }]);
  const fixture = fetchFixture({ accounts, playlist });
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' }, session: { ...SESSION, origin: 'https://music.youtube.com', clientName: 'WEB_REMIX' } }), fixture);
  assert.equal(result.ok, true);
  assert.equal(result.result.playlist.title, 'Fixture list');
  assert.equal(result.result.entries[0].trackId, 'abcdefghijk');
  assert.equal(result.result.entries[0].durationSeconds, 194);
  assert.match(result.result.nextCursor, /^yt-playlist-v2:/);
  assert.equal(JSON.parse(decodeURIComponent(result.result.nextCursor.slice('yt-playlist-v2:'.length))).token, 'next-page');
  assert.deepEqual(fixture.calls.map(call => new URL(call.url).pathname), ['/youtubei/v1/account/accounts_list', '/youtubei/v1/navigation/resolve_url', '/youtubei/v1/account/account_menu', '/youtubei/v1/navigation/resolve_url', '/youtubei/v1/browse']);
  assert.equal(fixture.calls.at(-1).body.context.user.onBehalfOfUser, undefined);
  assert.equal(new URL(fixture.calls.at(-1).url).origin, 'https://www.youtube.com');
  assert.equal(fixture.calls.at(-1).headers.get('x-youtube-client-name'), '1');
  assert.equal(fixture.calls.at(-1).body.context.client.clientName, 'WEB');
});

test('General YouTube requests preserve the exact captured profile selector', async () => {
  const playlist = { header: { playlistHeaderRenderer: { title: { simpleText: 'Fixture list' } } }, contents: { playlistVideoListRenderer: { contents: [] } } };
  const fixture = fetchFixture({ playlist });
  const dataSyncId = 'V1:persisted-context';
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' }, session: { ...SESSION, dataSyncId } }), fixture);
  assert.equal(result.ok, true);
  assert.equal(fixture.calls[0].body.context.user.onBehalfOfUser, undefined);
  assert.equal(fixture.calls.at(-1).body.context.user.onBehalfOfUser, dataSyncId);
  assert.equal(fixture.calls.at(-1).headers.get('x-goog-pageid'), dataSyncId);
});

test('General owner handle resolution omits selector while profile-bound browse keeps it', async () => {
  const playlist = { header: { musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLfixture123', ownerId: '@fixture' } }, contents: { playlistVideoListRenderer: { contents: [] } } };
  const fixture = fetchFixture({ playlist });
  const dataSyncId = 'V1:persisted-context';
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' }, session: { ...SESSION, dataSyncId } }), fixture);
  assert.equal(result.ok, true, JSON.stringify(result));
  const resolveCall = fixture.calls.find(call => new URL(call.url).pathname.endsWith('/resolve_url'));
  assert.ok(resolveCall);
  assert.equal(new URL(resolveCall.url).origin, 'https://www.youtube.com');
  assert.equal(resolveCall.body.context.user.onBehalfOfUser, undefined);
  assert.equal(fixture.calls.at(-1).body.context.user.onBehalfOfUser, dataSyncId);
  assert.equal(fixture.calls.at(-1).headers.get('x-goog-pageid'), dataSyncId);
});

test('YouTube getPlaylist does not mark an empty-looking unknown-count page complete', async () => {
  const playlist = {
    header: { playlistHeaderRenderer: { title: { simpleText: 'Unknown total' } } },
    contents: { playlistVideoListRenderer: { contents: [] } },
  };
  const fixture = fetchFixture({ playlist });
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fixture);
  assert.equal(result.ok, true);
  assert.equal(result.result.total, null);
  assert.equal(result.result.complete, false);
});

test('YouTube getPlaylist reports editable only for a matching playlist owned by the verified channel', async () => {
  const makePlaylist = ownerId => ({
    header: { musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLfixture123', ...(ownerId ? { ownerId } : {}) }, musicDetailHeaderRenderer: { secondSubtitle: { simpleText: '0 videos' } } },
    contents: { playlistVideoListRenderer: { contents: [] } },
  });
  for (const [ownerId, expected] of [[ACCOUNT_ID, true], ['UCbbbbbbbbbbbbbbbbbbbbbb', false], [null, false]]) {
    const fixture = fetchFixture({ playlist: makePlaylist(ownerId) });
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fixture);
    assert.equal(result.ok, true);
    assert.equal(result.result.playlist.canEdit, expected);
  }
});

test('YouTube worker scopes credentials to the exact origin and returns rate-limit metadata without body text', async () => {
  const fixture = fetchFixture();
  fixture.fetchImpl = async (url, options = {}) => {
    const parsed = new URL(url);
    fixture.calls.push({ url: parsed.href, method: options.method, headers: new Headers(options.headers) });
    if (parsed.pathname === '/youtubei/v1/account/account_menu') return response(webAccountMenu);
    if (parsed.pathname === '/youtubei/v1/account/accounts_list') return response(selectedAccount);
    if (parsed.pathname === '/youtubei/v1/navigation/resolve_url') return response({ endpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } });
    return response('private provider body must not escape', 429, { 'retry-after': '7' });
  };
  const result = await executeYouTubeJob(job({ operation: 'search', args: { query: 'fixture', kind: 'track' } }), fixture);
  assert.equal(result.error.code, 'RATE_LIMITED');
  assert.equal(result.error.phase, 'innertube');
  assert.equal(result.error.httpStatus, 429);
  assert.equal(result.error.retryAfterSeconds, 7);
  assert.equal(JSON.stringify(result).includes('private provider body'), false);
});


test('Music account menu fallback requires one active header and resolves its stable channel', async () => {
  for (const count of [0, 1, 2]) {
    const fixture = fetchFixture({accounts:{responseContext:{}}});
    const base = fixture.fetchImpl;
    fixture.fetchImpl = async (url, options) => {
      if (new URL(url).pathname.endsWith('/account/account_menu')) return response({actions:Array.from({length:count},()=>({openPopupAction:{popup:{multiPageMenuRenderer:{header:{activeAccountHeaderRenderer:{accountName:{simpleText:'Fixture Channel'},channelHandle:{simpleText:'@fixture'}}}}}}}))});
      return base(url, options);
    };
    const result = await executeYouTubeJob(job(), fixture);
    assert.equal(result.ok, count===1, JSON.stringify(result));
    if (count===1) assert.equal(result.result.accountId, ACCOUNT_ID);
    else assert.equal(result.error.code, 'SIGN_IN_REQUIRED');
  }
});


test('SAPISID-family credentials use the matching authorization scheme', async () => {
  for (const [name, schemes] of [['SAPISID',['SAPISIDHASH']], ['__Secure-1PAPISID',['SAPISID1PHASH']], ['__Secure-3PAPISID',['SAPISIDHASH','SAPISID3PHASH']]]) {
    const value='fixture-secret-for-sid'; const fixture=fetchFixture();
    const result=await executeYouTubeJob(job({session:{...SESSION,cookieHeader:`${name}=${value}`}}),fixture);
    assert.equal(result.ok,true,JSON.stringify(result));
    const auth=fixture.calls[0].headers.get('authorization');
    const parts=auth.split(' ');assert.deepEqual(parts.filter((_,i)=>i%2===0),schemes);
    for(let i=1;i<parts.length;i+=2){const [timestamp,hash]=parts[i].split('_');assert.equal(hash,createHash('sha1').update(`${timestamp} ${value} ${SESSION.origin}`).digest('hex'));}
    assert.equal(JSON.stringify(result).includes(value),false);
  }
});

test('all three allowed SAPISID-family cookies are forwarded and hashed independently', async () => {
  const values = {
    SAPISID: 'fixture-primary-cookie==',
    '__Secure-1PAPISID': 'fixture-first-party-cookie==',
    '__Secure-3PAPISID': 'fixture-third-party-cookie==',
  };
  const cookieHeader = Object.entries(values).map(([name, value]) => `${name}=${value}`).join('; ');
  const fixture = fetchFixture();
  const result = await executeYouTubeJob(job({ session: { ...SESSION, cookieHeader } }), fixture);
  assert.equal(result.ok, true, JSON.stringify(result));
  for (const call of fixture.calls) {
    assert.equal(call.headers.get('cookie'), cookieHeader);
    const authorization = call.headers.get('authorization');
    const timestamp = /^(?:SAPISIDHASH|SAPISID1PHASH|SAPISID3PHASH) (\d{1,12})_/.exec(authorization)?.[1];
    const expected = [
      ['SAPISIDHASH', values.SAPISID],
      ['SAPISID1PHASH', values['__Secure-1PAPISID']],
      ['SAPISID3PHASH', values['__Secure-3PAPISID']],
    ].map(([scheme, value]) => `${scheme} ${timestamp}_${createHash('sha1').update(`${timestamp} ${value} ${SESSION.origin}`).digest('hex')}`).join(' ');
    assert.equal(authorization, expected);
  }
  for (const value of Object.values(values)) assert.equal(JSON.stringify(result).includes(value), false);
});

test('allowed session cookies are forwarded while authorization hashes use only SAPISID-family proofs', async () => {
  const values = {
    SAPISID: 'fixture-primary-cookie==',
    '__Secure-1PAPISID': 'fixture-first-party-cookie==',
    '__Secure-3PAPISID': 'fixture-third-party-cookie==',
    LOGIN_INFO: 'fixture-login-info-cookie',
    SID: 'fixture-sid-cookie',
    HSID: 'fixture-hsid-cookie',
    SSID: 'fixture-ssid-cookie',
    APISID: 'fixture-apisid-cookie',
    '__Secure-1PSID': 'fixture-first-party-session',
    '__Secure-3PSID': 'fixture-third-party-session',
    SIDCC: 'fixture-sidcc-cookie',
    '__Secure-1PSIDTS': 'fixture-first-party-ts',
    '__Secure-3PSIDTS': 'fixture-third-party-ts',
    '__Secure-1PSIDCC': 'fixture-first-party-cc',
    '__Secure-3PSIDCC': 'fixture-third-party-cc',
  };
  const cookieHeader = Object.entries(values).map(([name, value]) => `${name}=${value}`).join('; ');
  const fixture = fetchFixture();
  const result = await executeYouTubeJob(job({ session: { ...SESSION, cookieHeader } }), fixture);
  assert.equal(result.ok, true, JSON.stringify(result));
  for (const call of fixture.calls) {
    assert.equal(call.headers.get('cookie'), cookieHeader);
    const authorization = call.headers.get('authorization');
    const parts = authorization.split(' ');
    assert.deepEqual(parts.filter((_, index) => index % 2 === 0), ['SAPISIDHASH', 'SAPISID1PHASH', 'SAPISID3PHASH']);
    const timestamp = parts[1].split('_')[0];
    for (const [index, value] of [values.SAPISID, values['__Secure-1PAPISID'], values['__Secure-3PAPISID']].entries()) {
      const actualHash = parts[index * 2 + 1].split('_')[1];
      assert.equal(actualHash, createHash('sha1').update(`${timestamp} ${value} ${SESSION.origin}`).digest('hex'));
    }
  }
  for (const value of Object.values(values)) assert.equal(JSON.stringify(result).includes(value), false);
});

test('YouTube session rejects malformed or non-allowlisted multi-cookie headers before networking', async () => {
  const invalidHeaders = [
    'LOGIN_INFO=only-nonproof-cookies',
    `${COOKIE}; NID=unrelated-google-cookie`,
    `${COOKIE}; SAPISID=duplicate-cookie-value`,
    `${COOKIE}; __Secure-1PAPISID=has whitespace`,
    `${COOKIE} `,
    `${COOKIE}; __Secure-3PAPISID=short`,
    `${COOKIE}; __Secure-3PAPISID=bad\rvalue`,
  ];
  for (const cookieHeader of invalidHeaders) {
    const fixture = fetchFixture();
    const result = await executeYouTubeJob(job({ session: { ...SESSION, cookieHeader } }), fixture);
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'UNSUPPORTED');
    assert.equal(fixture.calls.length, 0);
  }
});

test('worker redacts every supplied session-cookie value from normalized output', async () => {
  const values = ['fixture-primary-secret', 'fixture-first-party-secret', 'fixture-third-party-secret', 'fixture-login-info-secret', 'fixture-sid-secret'];
  const cookieHeader = [`SAPISID=${values[0]}`, `__Secure-1PAPISID=${values[1]}`, `__Secure-3PAPISID=${values[2]}`, `LOGIN_INFO=${values[3]}`, `SID=${values[4]}`].join('; ');
  const fixture = fetchFixture();
  const originalFetch = fixture.fetchImpl;
  fixture.fetchImpl = async (url, options = {}) => {
    if (new URL(url).pathname.endsWith('/accounts_list')) {
      return response(accountResponse([{ isSelected: true, accountName: { simpleText: values[3] }, channelHandle: { simpleText: '@fixture' } }]));
    }
    return originalFetch(url, options);
  };
  const result = await executeYouTubeJob(job({ session: { ...SESSION, cookieHeader } }), fixture);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'PROVIDER_CHANGED');
  for (const value of values) assert.equal(JSON.stringify(result).includes(value), false);
});


test('General content fails before browse when WWW active channel differs', async () => {
  const fixture = fetchFixture({ playlist: { contents: {} } });
  const fallback = fixture.fetchImpl;
  fixture.fetchImpl = async (url, options) => {
    const path = new URL(url).pathname;
    if (path.endsWith('/account_menu')) return response({ actions: [{ openPopupAction: { popup: { multiPageMenuRenderer: { header: { activeAccountHeaderRenderer: { channelHandle: { simpleText: '@other' } } } } } } }] });
    if (path.endsWith('/resolve_url') && JSON.parse(options.body).url.endsWith('/@other')) return response({ endpoint: { browseEndpoint: { browseId: 'UCbbbbbbbbbbbbbbbbbbbbbb' } } });
    return fallback(url, options);
  };
  for (const operation of ['listPlaylists', 'getPlaylist', 'applyPlaylistEdits', 'search']) {
    const result = await executeYouTubeJob(job({ operation, args: { playlistId: 'PLfixture123', query: 'fixture' }, session: { ...SESSION, origin: 'https://music.youtube.com', clientName: 'WEB_REMIX' } }), fixture);
    assert.equal(result.error.code, 'ACCOUNT_CHANGED');
  }
  assert.equal(fixture.calls.some(call => new URL(call.url).pathname === '/youtubei/v1/browse'), false);
});

test('General playlist header proves owned editable WEB playlists', async () => {
  const fixture = fetchFixture({ playlist: { header: { playlistHeaderRenderer: { playlistId: 'PLfixture123', isEditable: true, ownerEndpoint: { browseEndpoint: { browseId: ACCOUNT_ID } }, title: { simpleText: 'General videos' }, numVideosText: { simpleText: '0 videos' } } }, contents: { playlistVideoListRenderer: { contents: [] } } } });
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fixture);
  assert.equal(result.ok, true);
  assert.equal(result.result.playlist.canEdit, true);
  assert.equal(result.result.complete, true);
});

const CREATED_VERIFICATION_ID = 'PLverification1234567890';
test('Music session creates fixed empty private playlist on WWW with verified active channel', async () => {
  const fixture = fetchFixture(), base = fixture.fetchImpl;
  let writes = 0;
  fixture.fetchImpl = async (url, options) => {
    if (new URL(url).pathname !== '/youtubei/v1/playlist/create') return base(url, options);
    writes++;
    assert.equal(new URL(url).origin, 'https://www.youtube.com');
    const { context, ...body } = JSON.parse(options.body);
    assert.deepEqual(body, { title: 'Burrata verification', privacyStatus: 'PRIVATE', videoIds: [] });
    assert.equal(context.client.clientName, 'WEB');
    assert.equal(context.user.onBehalfOfUser, 'music-context');
    assert.equal(new Headers(options.headers).get('x-goog-pageid'), 'music-context');
    return response({ playlistId: CREATED_VERIFICATION_ID });
  };
  const result = await executeYouTubeJob(job({ creationRequestId: '0123456789abcdef0123456789abcdef', operation: 'createVerificationPlaylist', args: { accountId: ACCOUNT_ID }, session: { ...SESSION, origin: 'https://music.youtube.com', clientName: 'WEB_REMIX', dataSyncId: 'music-context' } }), fixture);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.status, 'CREATED_REQUIRES_READBACK');
  assert.equal(writes, 1);
});
test('YouTube creation blocks caller overrides and returns unknown after response loss or unsafe normalization', async () => {
  for (const args of [{ accountId: 'UCbbbbbbbbbbbbbbbbbbbbbb' }, { accountId: ACCOUNT_ID, privacyStatus: 'PUBLIC' }, { accountId: ACCOUNT_ID, videoIds: ['12345678901'] }]) {
    const fixture = fetchFixture();
    const result = await executeYouTubeJob(job({ creationRequestId: '0123456789abcdef0123456789abcdef', operation: 'createVerificationPlaylist', args }), fixture);
    assert.equal(result.ok, false);
    assert.equal(fixture.calls.some(call => new URL(call.url).pathname === '/youtubei/v1/playlist/create'), false);
  }
  for (const behavior of ['throw', 'bad-id', 'redacted-id']) {
    const fixture = fetchFixture(), base = fixture.fetchImpl;
    let writes = 0;
    fixture.fetchImpl = async (url, options) => {
      if (new URL(url).pathname !== '/youtubei/v1/playlist/create') return base(url, options);
      writes++;
      if (behavior === 'throw') throw new Error('response lost');
      return response({ playlistId: behavior === 'bad-id' ? 'bad' : 'PLfixture-secret-cookie-value' });
    };
    const result = await executeYouTubeJob(job({ creationRequestId: '0123456789abcdef0123456789abcdef', operation: 'createVerificationPlaylist', args: { accountId: ACCOUNT_ID } }), fixture);
    assert.equal(result.error.code, 'OUTCOME_UNKNOWN', JSON.stringify(result));
    assert.equal(writes, 1);
  }
});
test('YouTube WWW readback exposes provider owner and privacy while preserving unknown counts', async () => {
  for (const exactCount of [true, false]) {
    const playlist = {
      header: { playlistHeaderRenderer: { playlistId: CREATED_VERIFICATION_ID, title: { simpleText: 'Burrata verification' }, ownerEndpoint: { browseEndpoint: { browseId: ACCOUNT_ID } }, privacy: 'PRIVATE', isEditable: true, ...(exactCount ? { numVideosText: { simpleText: '0 videos' } } : {}) } },
      contents: { playlistVideoListRenderer: { contents: [] } },
    };
    const fixture = fetchFixture({ playlist });
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: CREATED_VERIFICATION_ID } }), fixture);
    assert.equal(result.ok, true, JSON.stringify(result));
    assert.equal(result.result.playlist.ownerId, ACCOUNT_ID);
    assert.equal(result.result.playlist.visibility, 'private');
    assert.equal(result.result.total, exactCount ? 0 : null);
    assert.equal(result.result.complete, exactCount);
  }
});


test('General WWW identity uses only captured profile selector and independently verifies its active channel', async () => {
  const fixture = fetchFixture({ accounts: { responseContext: {} }, playlist: { contents: { richGridRenderer: { contents: [] } } } });
  const base = fixture.fetchImpl;
  let menus = 0;
  fixture.fetchImpl = async (url, options) => {
    if (new URL(url).pathname.endsWith('/account_menu')) {
      menus++;
      assert.equal(new URL(url).origin, 'https://www.youtube.com');
      const body = JSON.parse(options.body);
      assert.equal(body.context.user.onBehalfOfUser, 'captured-profile');
      assert.equal(new Headers(options.headers).get('x-goog-pageid'), 'captured-profile');
      return response(webAccountMenu);
    }
    return base(url, options);
  };
  const result = await executeYouTubeJob(job({ operation: 'listPlaylists', session: { ...SESSION, origin: 'https://music.youtube.com', clientName: 'WEB_REMIX', dataSyncId: 'captured-profile' } }), fixture);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.accountId, ACCOUNT_ID);
  assert.equal(menus, 2);
});


test('YouTube creation preserves finite failure phase and HTTP status without provider body', async () => {
  for (const [status, phase] of [[400, 'playlist_creation'], [403, 'playlist_creation']]) {
    const fixture = fetchFixture(), base = fixture.fetchImpl;
    fixture.fetchImpl = async (url, options) => new URL(url).pathname === '/youtubei/v1/playlist/create'
      ? response('private provider response must not escape', status) : base(url, options);
    const result = await executeYouTubeJob(job({ creationRequestId: '0123456789abcdef0123456789abcdef', operation: 'createVerificationPlaylist', args: { accountId: ACCOUNT_ID } }), fixture);
    assert.equal(result.error.code, 'OUTCOME_UNKNOWN');
    assert.equal(result.error.phase, phase);
    assert.equal(result.error.httpStatus, status);
    assert.doesNotMatch(JSON.stringify(result), /private provider response/);
  }
  const fixture = fetchFixture(), base = fixture.fetchImpl;
  fixture.fetchImpl = async (url, options) => new URL(url).pathname === '/youtubei/v1/playlist/create'
    ? response({ unsupported: 'private response content' }) : base(url, options);
  const result = await executeYouTubeJob(job({ creationRequestId: '0123456789abcdef0123456789abcdef', operation: 'createVerificationPlaylist', args: { accountId: ACCOUNT_ID } }), fixture);
  assert.equal(result.error.code, 'OUTCOME_UNKNOWN');
  assert.equal(result.error.phase, 'playlist_creation_response');
  assert.doesNotMatch(JSON.stringify(result), /private response content/);
});

test('Large YouTube source pages keep all occurrences, provider continuation and incomplete state', async () => {
  const playlist = {
    header: { playlistHeaderRenderer: { title: { simpleText: 'Large fixture' }, numVideosText: { simpleText: '251 videos' } } },
    contents: { playlistVideoListRenderer: { contents: [
      ...Array.from({ length: 250 }, (_, index) => ({ playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: `entry-${index}`, title: { simpleText: 'Repeated video' } } })),
      { continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'source-next' } } } },
    ] } },
  };
  const fixture = fetchFixture({ playlist });
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fixture);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.entries.length, 250);
  assert.equal(result.result.entries[249].entryId, 'entry-249');
  assert.equal(result.result.complete, false);
  const cursor = JSON.parse(decodeURIComponent(result.result.nextCursor.slice('yt-playlist-v2:'.length)));
  assert.equal(cursor.token, 'source-next');
});

// Structural fixtures derived from retained YouTube.js 18.1 parser contracts.
const playlistPageFixtures = JSON.parse(await readFile(new URL('./fixtures/youtube-playlist-pages.json', import.meta.url), 'utf8'));
const clonePlaylistFixture = value => JSON.parse(JSON.stringify(value));

test('Playlist sidebar stat zero supplies exact count; item-list cursor wins over recommendations', async () => {
  const fixture = fetchFixture({ playlist: playlistPageFixtures.initial });
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fixture);
  assert.equal(result.ok, true);
  assert.equal(result.result.total, 4);
  assert.deepEqual(Array.from(result.result.entries, row => row.entryId), ['entry-1', 'entry-2']);
  assert.deepEqual(Array.from(result.result.entries, row => row.trackId), ['abcdefghijk', 'abcdefghijk']);
  const cursor = JSON.parse(decodeURIComponent(result.result.nextCursor.slice('yt-playlist-v2:'.length)));
  assert.equal(cursor.token, 'playlist-next');
  assert.equal(cursor.accountId, ACCOUNT_ID);
  assert.equal(cursor.playlistId, 'PLfixture123');
  assert.equal(cursor.origin, 'https://www.youtube.com');
  assert.equal(result.result.complete, false);
});

test('Playlist append action envelopes preserve duplicate/unavailable occurrences without recommendations', async () => {
  for (const key of ['onResponseReceivedActions', 'onResponseReceivedEndpoints', 'onResponseReceivedCommands']) {
    const page = { [key]: playlistPageFixtures.append.onResponseReceivedActions };
    const fixture = fetchFixture({ playlist: page });
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fixture);
    assert.equal(result.ok, true);
    assert.deepEqual(Array.from(result.result.entries, row => row.entryId), ['entry-3', 'entry-deleted']);
    assert.equal(result.result.entries[0].trackId, 'abcdefghijk');
    assert.equal(result.result.entries[1].trackId, null);
    assert.equal(result.result.entries[1].available, false);
    assert.equal(result.result.nextCursor, null);
    assert.equal(result.result.total, null);
    assert.equal(result.result.complete, false, 'a terminal page does not prove global completeness');
  }
});

test('Playlist continuation cursor stays bound to playlist even with append actions', async () => {
  const fixture = fetchFixture();
  const original = fixture.fetchImpl;
  const browse = [];
  fixture.fetchImpl = async (url, options) => {
    if (new URL(url).pathname !== '/youtubei/v1/browse') return original(url, options);
    const body = JSON.parse(options.body); browse.push(body);
    return response(body.continuation ? playlistPageFixtures.append : playlistPageFixtures.initial);
  };
  const first = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fixture);
  const second = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', cursor: first.result.nextCursor } }), fixture);
  assert.equal(second.ok, true);
  assert.equal(browse[1].continuation, 'playlist-next');
  const wrong = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLdifferent123', cursor: first.result.nextCursor } }), fixture);
  assert.equal(wrong.error.code, 'UNSUPPORTED');
  assert.equal(browse.length, 2);
});

test('Sidebar views/abbreviated or missing totals never turn unknown pages complete', async () => {
  for (const stat of [undefined, { simpleText: '1.2K videos' }, { simpleText: '999 views' }]) {
    const playlist = { sidebar: { playlistSidebarRenderer: { items: [{ playlistSidebarPrimaryInfoRenderer: { stats: [stat, { simpleText: '0' }] } }] } }, contents: { playlistVideoListRenderer: { contents: [] } } };
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
    assert.equal(result.ok, true);
    assert.equal(result.result.total, null);
    assert.equal(result.result.complete, false);
  }
  const playlist = { sidebar: { playlistSidebarRenderer: { items: [{ playlistSidebarPrimaryInfoRenderer: { stats: [{ simpleText: '0 videos' }, { simpleText: '999 views' }] } }] } }, contents: { playlistVideoListRenderer: { contents: [] } }, recommendations: { playlistVideoRenderer: { videoId: 'abcdefghijk', style: 'PLAYLIST_VIDEO_RENDERER_STYLE_RECOMMENDED_VIDEO' } } };
  const zero = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
  assert.equal(zero.result.total, 0);
  assert.equal(zero.result.entries.length, 0);
  assert.equal(zero.result.complete, true);
});

test('Conflicting list cursors and unrelated append targets cannot imply completeness', async () => {
  const continuation = token => ({ continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token } } } });
  const playlist = { header: { playlistHeaderRenderer: { numVideosText: { simpleText: '0 videos' } } }, contents: { playlistVideoListRenderer: { contents: [continuation('one'), continuation('two')] } } };
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
  assert.equal(result.result.nextCursor, null);
  assert.equal(result.result.complete, false);
  const unrelated = { onResponseReceivedActions: [{ appendContinuationItemsAction: { target: 'PLdifferent123', continuationItems: [playlistPageFixtures.initial.contents.sectionListRenderer.contents[0].playlistVideoListRenderer.contents[0], continuation('wrong')] } }] };
  const skipped = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist: unrelated }));
  assert.equal(skipped.result.entries.length, 0);
  assert.equal(skipped.result.nextCursor, null);
  assert.equal(skipped.result.complete, false);
});

test('Reviewed edit reads append continuation occurrences and excludes recommendations before comparison', async () => {
  const initial = clonePlaylistFixture(playlistPageFixtures.initial);
  initial.header.musicEditablePlaylistDetailHeaderRenderer = { playlistId: 'PLfixture123', ownerId: ACCOUNT_ID };
  const fixture = fetchFixture(); const original = fixture.fetchImpl;
  let edited = false;
  fixture.fetchImpl = async (url, options) => {
    const path = new URL(url).pathname;
    if (path === '/youtubei/v1/browse/edit_playlist') { edited = true; return response({ status: 'STATUS_SUCCEEDED' }); }
    if (path === '/youtubei/v1/browse') return response(JSON.parse(options.body).continuation ? playlistPageFixtures.append : initial);
    return original(url, options);
  };
  const expectedEntries = ['entry-1', 'entry-2', 'entry-3', 'entry-deleted'].map(entryId => ({ entryId, trackId: entryId === 'entry-deleted' ? null : 'abcdefghijk' }));
  const result = await executeYouTubeJob(job({ operation: 'applyPlaylistEdits', args: { previewApproved: true, playlistId: 'PLfixture123', expectedEntries, actions: [{ type: 'remove', entryId: 'entry-2', trackId: 'abcdefghijk' }] } }), fixture);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(edited, true);
});


test('Scoped Music shelves and nested append commands retain valid repeated Music rows', async () => {
  const music = entryId => ({ musicResponsiveListItemRenderer: { playlistItemData: { videoId: 'abcdefghijk', setVideoId: entryId }, flexColumns: [{ musicResponsiveListItemFlexColumnRenderer: { text: { simpleText: 'Music occurrence' } } }] } });
  const playlist = { header: { musicDetailHeaderRenderer: { secondSubtitle: { simpleText: '2 songs' } } }, contents: { musicPlaylistShelfRenderer: { playlistId: 'PLfixture123', contents: [music('music-one'), music('music-two')], continuations: [{ nextContinuationData: { continuation: 'music-next' } }] } }, recommendations: { musicShelfRenderer: { contents: [music('not-in-playlist')], continuations: [{ nextContinuationData: { continuation: 'recommendation-next' } }] } } };
  const first = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
  assert.equal(first.ok, true);
  assert.deepEqual(Array.from(first.result.entries, row => row.entryId), ['music-one', 'music-two']);
  assert.equal(JSON.parse(decodeURIComponent(first.result.nextCursor.slice('yt-playlist-v2:'.length))).token, 'music-next');
  const nested = { onResponseReceivedEndpoints: [{ commandExecutorCommand: { commands: [{ appendContinuationItemsAction: { target: 'VLPLfixture123', continuationItems: [music('music-three'), music('music-four')] } }] } }] };
  const second = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist: nested }));
  assert.equal(second.ok, true);
  assert.deepEqual(Array.from(second.result.entries, row => row.entryId), ['music-three', 'music-four']);
  assert.equal(second.result.complete, false);
});

test('Ambiguous recognized paging stops approved mutation before any edit request', async () => {
  const continuation = token => ({ continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token } } } });
  const playlist = { header: { musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLfixture123', ownerId: ACCOUNT_ID }, playlistHeaderRenderer: { numVideosText: { simpleText: '0 videos' } } }, contents: { playlistVideoListRenderer: { contents: [continuation('one'), continuation('two')] } } };
  const fixture = fetchFixture({ playlist });
  const result = await executeYouTubeJob(job({ operation: 'applyPlaylistEdits', args: { previewApproved: true, playlistId: 'PLfixture123', expectedEntries: [], actions: [{ type: 'add', trackId: 'abcdefghijk' }] } }), fixture);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'PROVIDER_CHANGED');
  assert.equal(fixture.calls.some(call => new URL(call.url).pathname.endsWith('/edit_playlist')), false);
});


test('Append item-list cursor remains scoped ahead of recommendation and sidebar tokens', async () => {
  const playlist = clonePlaylistFixture(playlistPageFixtures.append);
  playlist.onResponseReceivedActions[0].appendContinuationItemsAction.continuationItems.push({ continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'append-next' } } } });
  playlist.sidebar = { continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'sidebar-not-playlist' } } } };
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  const cursor = JSON.parse(decodeURIComponent(result.result.nextCursor.slice('yt-playlist-v2:'.length)));
  assert.deepEqual(cursor, { accountId: ACCOUNT_ID, playlistId: 'PLfixture123', origin: 'https://www.youtube.com', token: 'append-next', expectedTotal: null, offset: 2 });
  assert.equal(result.result.complete, false);
});


test('Explicit unrelated append targets cannot borrow ordinary playlist row shapes', async () => {
  const row = playlistPageFixtures.initial.contents.sectionListRenderer.contents[0].playlistVideoListRenderer.contents[0];
  const cursor = { continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'unrelated-next' } } } };
  for (const route of [{ target: 'recommendations' }, { target: 'search-results' }, { target: 'playlist-video-list-other' }, { targetId: 'recommendations' }, { target: null }, { target: '' }, { target: 'playlist-video-list', targetId: 'recommendations' }]) {
    const playlist = { header: { playlistHeaderRenderer: { numVideosText: { simpleText: '0 videos' } } }, onResponseReceivedActions: [{ appendContinuationItemsAction: { ...route, continuationItems: [row, cursor] } }] };
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
    assert.equal(result.ok, true);
    assert.equal(result.result.entries.length, 0, JSON.stringify(route));
    assert.equal(result.result.nextCursor, null);
    assert.equal(result.result.complete, false);
  }
});

test('Untargeted provider-shaped append actions preserve playlist occurrences', async () => {
  const playlist = clonePlaylistFixture(playlistPageFixtures.append);
  delete playlist.onResponseReceivedActions[0].appendContinuationItemsAction.target;
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  assert.deepEqual(Array.from(result.result.entries, row => row.entryId), ['entry-3', 'entry-deleted']);
  assert.equal(result.result.complete, false);
});

test('Playlist response shape counts nested fixed structures without reflecting hostile response data', async () => {
  const secret = COOKIE.split('=')[1];
  const playlist = JSON.parse(JSON.stringify({ contents: { playlistVideoListRenderer: { contents: [] } }, continuationContents: { sectionListContinuation: { contents: [{ itemSectionContinuation: { contents: [{ richGridContinuation: { items: [{ lockupViewModel: { contentId: secret } }] } }] } }] } }, onResponseReceivedCommands: [{ commandExecutorCommand: { commands: [{ appendContinuationItemsAction: { target: secret, continuationItems: [] } }, { reloadContinuationItemsCommand: {} }] } }], [secret]: { privateRawTitle: secret }, sidebar: { arbitraryPrivateKey: secret } }));
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  const shape = result.result.responseShape;
  assert.equal(shape.version, 1);
  assert.equal(shape.counts.continuationContents.sectionListContinuation, 1);
  assert.equal(shape.counts.continuationContents.itemSectionContinuation, 1);
  assert.equal(shape.counts.continuationContents.richGridContinuation, 1);
  assert.equal(shape.counts.continuationContents.lockupViewModel, 1);
  assert.equal(shape.counts.onResponseReceivedCommands.reloadContinuationItemsCommand, 1);
  assert.equal(shape.appendTargets.other, 1);
  assert.equal(shape.recognizedSections, 1);
  assert.equal(shape.recognizedCursor, false);
  assert.equal(shape.ambiguous, false);
  assert.equal(shape.truncated, false);
  assert.equal(JSON.stringify(shape).includes(secret), false);
  assert.equal(JSON.stringify(shape).includes('privateRawTitle'), false);
  assert.equal(JSON.stringify(shape).includes('arbitraryPrivateKey'), false);
});

test('Playlist diagnostic reports only bounded empty-count and sidebar identity evidence for a page-header shell', async () => {
  const rawId = 'PL_RAW_SECRET_123';
  const rawTitle = 'PRIVATE_FIXTURE_TITLE';
  const rawToken = 'PRIVATE_FIXTURE_CONTINUATION_TOKEN';
  const playlist = {
    header: {
      pageHeaderRenderer: { pageTitle: { simpleText: rawTitle }, content: { pageHeaderViewModel: { metadata: { contentMetadataViewModel: { metadataRows: [] } } } } },
      commandExecutorCommand: { commands: [{ openPopupAction: { popup: { menu: { continuation: rawToken } } } }] },
    },
    sidebar: { playlistSidebarRenderer: { items: [
      { playlistSidebarPrimaryInfoRenderer: { stats: [{ simpleText: '0 videos' }, { simpleText: '999 views' }] } },
      { playlistSidebarSecondaryInfoRenderer: { ownerEndpoint: { browseEndpoint: { browseId: ACCOUNT_ID } }, privacy: 'PRIVATE', playlistId: rawId } },
    ] } },
    contents: { sectionListRenderer: { contents: [{ itemSectionRenderer: { contents: [] } }] } },
  };
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  assert.equal(result.result.total, 0, 'the sidebar count is explicit zero evidence');
  assert.deepEqual(Array.from(result.result.entries), []);
  assert.equal(result.result.complete, false, 'empty rows alone must not establish a recognized complete playlist item list');
  assert.equal(result.result.playlist.visibility, null, 'sidebar privacy is diagnostic evidence, not target-bound playlist privacy');
  assert.equal(result.result.playlist.ownerId, null, 'sidebar owner is diagnostic evidence, not target-bound playlist ownership');
  const shape = result.result.responseShape;
  assert.equal(shape.counts.header.pageHeaderRenderer, 1);
  assert.equal(shape.counts.header.commandExecutorCommand, 1);
  assert.equal(shape.counts.sidebar.playlistSidebarRenderer, 1);
  assert.equal(shape.counts.sidebar.playlistSidebarPrimaryInfoRenderer, 1);
  assert.equal(shape.counts.sidebar.playlistSidebarSecondaryInfoRenderer, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(shape.identityEvidence)), {
    canonicalRejectReason: 0,
    canonicalProtocolClass: 0,
    canonicalHostClass: 0,
    canonicalHasCredentials: 0,
    canonicalHasPort: 0,
    canonicalPathClass: 0,
    canonicalListCount: 0,
    canonicalListMatches: 0,
    editMatchedCommands: 0,
    editForeignCommands: 0,
    editMalformedCommands: 0,
    editApiPathMatched: 0,
    editParamsPresent: 0,
    editActionsMissing: 0,
    editActionsEmpty: 0,
    editActionsMalformed: 0,
    editActionAdd: 0,
    editActionRemove: 0,
    editActionMove: 0,
    editActionPrivacy: 0,
    editActionMetadata: 0,
    editActionOther: 0,
    editActionCollection: 0,
    editActionCollaboration: 0,
    editActionVoting: 0,
    editFlagTrue: 0,
    editFlagFalse: 0,
    editFlagMalformed: 0,
    editFlagUnbound: 0,
    editReorderTrue: 0,
    editTraversalTruncated: 0,
    emptyRejectReason: 3,
    emptyContinuationHeader: 0,
    emptyContinuationSidebar: 0,
    emptyContinuationContents: 0,
    emptyContinuationOther: 0,
    emptyMembershipNodes: 0,
    emptyTraversalTruncated: 0,
    modernPrimaryCount: 1,
    modernSecondaryCount: 1,
    modernMatchingIds: 0,
    modernConflictingIds: 0,
    modernMalformedEditIds: 0,
    modernMalformedTitleRuns: 0,
    modernMatchingCanonicals: 0,
    modernRejectedCanonicals: 0,
    modernTraversalTruncated: 0,
    modernOwnerRuns: 0,
    modernMatchingOwners: 0,
    modernConflictingOwners: 0,
    modernMalformedOwnerRuns: 0,
    modernSelectedPrivacyCount: 0,
    modernTargetBound: 0,
    modernIdentityRecognized: 0,
    modernSelectedTabs: 0,
    modernMemberSections: 0,
    modernMessageOnlySection: 0,
    modernEmptyRecognized: 0,
    targetHeaders: 0, primaryPlaylistIds: 0, primaryNavigationPlaylistIds: 0, canonicalPlaylistIds: 0, editEndpointPlaylistIds: 0,
    privacyFieldPresent: 0, privateHeaders: 0, publicHeaders: 0, unlistedHeaders: 0, unknownPrivacyHeaders: 0,
    ownerEndpoints: 0, ownerMatchesConfirmedAccount: 0, ownerMismatches: 0,
    sidebarOwnerEndpoints: 1, sidebarOwnerMatchesConfirmedAccount: 1, sidebarOwnerMismatches: 0,
    privacyDropdownEntries: 0, privacyDropdownSelectedPrivate: 0, privacyDropdownSelectedPublic: 0, privacyDropdownSelectedUnlisted: 0, privacyDropdownSelectedUnknown: 0,
    sidebarVideoOwnerNodes: 0, sidebarVideoOwnerBrowseEndpoints: 0, sidebarVideoOwnerMatchesConfirmedAccount: 0, sidebarVideoOwnerMismatches: 0,
    sidebarPrivacyFields: 1, sidebarPrivateValues: 1, sidebarPublicValues: 0, sidebarUnlistedValues: 0, sidebarUnknownPrivacyValues: 0,
  });
  assert.deepEqual(JSON.parse(JSON.stringify(shape.totalEvidence)), {
    sidebarPrimaryInfoRenderers: 1, sidebarStatsArrays: 1, sidebarFirstStatPresent: 1, sidebarFirstStatParseable: 1,
    sidebarFirstStatNumericZero: 1, sidebarFirstStatEnglishNoVideos: 0, sidebarFirstStatEnglishLocale: true,
    sidebarFirstStatTextShape: { string: 0, simpleText: 1, runs: 0, content: 0, dynamicTextViewModel: 0, textContent: 0, accessibilityLabel: 0, other: 0 },
    sidebarFirstStatZero: 1, exactTotalRecognized: true, exactTotalIsZero: true, truncated: false,
  });
  const serialized = JSON.stringify(shape);
  for (const secret of [rawId, rawTitle, rawToken, ACCOUNT_ID]) assert.equal(serialized.includes(secret), false, `diagnostic must not expose ${secret}`);
});

test('Only the explicit English zero-count phrase in sidebar primary-info stats[0] counts as empty', async () => {
  const playlist = {
    sidebar: { playlistSidebarRenderer: { items: [{ playlistSidebarPrimaryInfoRenderer: { stats: [{ simpleText: '  NO VIDEOS  ' }] } }] } },
    contents: { sectionListRenderer: { contents: [{ itemSectionRenderer: { contents: [] } }] } },
  };
  const run = async hl => executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true }, session: { ...SESSION, hl } }), fetchFixture({ playlist }));
  const english = await run('en-US');
  assert.equal(english.ok, true);
  assert.equal(english.result.total, 0);
  assert.equal(english.result.complete, false, 'an explicit zero does not replace a missing playlist-bound item list');
  assert.equal(english.result.responseShape.totalEvidence.sidebarFirstStatEnglishNoVideos, 1);
  assert.equal(english.result.responseShape.totalEvidence.sidebarFirstStatNumericZero, 0);
  assert.equal(JSON.stringify(english.result.responseShape).includes('NO VIDEOS'), false);
  const nonEnglish = await run('fr');
  assert.equal(nonEnglish.ok, true);
  assert.equal(nonEnglish.result.total, null, 'unknown locale wording remains unknown');
  assert.equal(nonEnglish.result.complete, false);
  assert.equal(nonEnglish.result.responseShape.totalEvidence.sidebarFirstStatEnglishLocale, false);
  assert.equal(nonEnglish.result.responseShape.totalEvidence.sidebarFirstStatEnglishNoVideos, 0);
});

test('Playlist diagnostics count known modern sidebar owner and page-header structures without exposing values', async () => {
  const rawOwner = ACCOUNT_ID, rawTitle = 'PRIVATE_HEADER_TITLE', rawTarget = 'PLfixture123';
  const playlist = {
    header: {
      pageHeaderRenderer: { pageTitle: { simpleText: rawTitle }, pageHeaderViewModel: { metadata: { contentMetadataViewModel: { metadataRows: [] } } } },
      commandExecutorCommand: {
        commands: [{
          openPopupAction: {
            popup: { menu: { items: [{ navigationEndpoint: { browseEndpoint: { browseId: rawOwner } } }] } },
          },
        }],
      },
    },
    sidebar: {
      playlistSidebarRenderer: { items: [{
        playlistSidebarSecondaryInfoRenderer: {
          videoOwner: { title: { runs: [{ text: 'Owner', navigationEndpoint: { browseEndpoint: { browseId: rawOwner } } }] } },
        },
      }] },
    },
    contents: { sectionListRenderer: { contents: [] } },
  };
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: rawTarget, inspectResponseShape: true } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  const shape = result.result.responseShape;
  assert.equal(shape.counts.header.pageHeaderRenderer, 1);
  assert.equal(shape.counts.header.pageHeaderViewModel, 1);
  assert.equal(shape.counts.header.commandExecutorCommand, 1);
  assert.equal(shape.counts.header.openPopupAction, 1);
  assert.equal(shape.counts.sidebar.playlistSidebarSecondaryInfoRenderer, 1);
  assert.equal(shape.counts.sidebar.videoOwner, 1);
  assert.equal(shape.counts.sidebar.navigationEndpoint, 1);
  assert.equal(shape.counts.sidebar.browseEndpoint, 1);
  assert.equal(shape.identityEvidence.sidebarVideoOwnerNodes, 1);
  assert.equal(shape.identityEvidence.sidebarVideoOwnerBrowseEndpoints, 1);
  assert.equal(shape.identityEvidence.sidebarVideoOwnerMatchesConfirmedAccount, 1);
  assert.equal(shape.identityEvidence.sidebarVideoOwnerMismatches, 0);
  for (const value of [rawOwner, rawTitle, rawTarget]) assert.equal(JSON.stringify(shape).includes(value), false);
});

test('Playlist diagnostic keeps absent, foreign, and malformed owner/privacy evidence distinct', async () => {
  const inspect = async header => {
    const playlist = { header: { pageHeaderRenderer: { pageTitle: { simpleText: 'redacted' } }, ...(header ? { musicPlaylistEditHeaderRenderer: header } : {}) }, contents: { sectionListRenderer: { contents: [{ itemSectionRenderer: { contents: [] } }] } } };
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
    assert.equal(result.ok, true);
    assert.equal(result.result.total, null);
    assert.equal(result.result.complete, false);
    return { result: result.result, shape: result.result.responseShape };
  };
  const absent = await inspect(null);
  assert.equal(absent.shape.identityEvidence.targetHeaders, 0);
  assert.equal(absent.shape.identityEvidence.ownerMatchesConfirmedAccount, 0);
  assert.equal(absent.shape.identityEvidence.privateHeaders, 0);
  assert.equal(absent.shape.totalEvidence.exactTotalRecognized, false);
  const foreign = await inspect({ playlistId: 'PLfixture123', privacy: 'PRIVATE', ownerEndpoint: { browseEndpoint: { browseId: 'UCbbbbbbbbbbbbbbbbbbbbbb' } } });
  assert.equal(foreign.shape.identityEvidence.targetHeaders, 1);
  assert.equal(foreign.shape.identityEvidence.ownerEndpoints, 1);
  assert.equal(foreign.shape.identityEvidence.ownerMatchesConfirmedAccount, 0);
  assert.equal(foreign.shape.identityEvidence.ownerMismatches, 1);
  assert.equal(foreign.result.playlist.ownerId, 'UCbbbbbbbbbbbbbbbbbbbbbb');
  assert.equal(foreign.result.playlist.canEdit, false);
  const malformed = await inspect({ playlistId: 'PLfixture123', privacy: 'UNLISTED_OR_UNKNOWN', ownerEndpoint: 'malformed-owner' });
  assert.equal(malformed.shape.identityEvidence.targetHeaders, 1);
  assert.equal(malformed.shape.identityEvidence.unknownPrivacyHeaders, 1);
  assert.equal(malformed.shape.identityEvidence.ownerEndpoints, 0);
  assert.equal(malformed.shape.identityEvidence.ownerMatchesConfirmedAccount, 0);
  assert.equal(malformed.result.playlist.visibility, null);
  assert.equal(malformed.result.playlist.ownerId, null);
  for (const { shape } of [absent, foreign, malformed]) {
    const serialized = JSON.stringify(shape);
    assert.equal(serialized.includes('PLfixture123'), false);
    assert.equal(serialized.includes('UCbbbbbbbbbbbbbbbbbbbbbb'), false);
    assert.equal(serialized.includes('UNLISTED_OR_UNKNOWN'), false);
    assert.equal(serialized.includes('malformed-owner'), false);
  }
});

test('Playlist response shape bounds counts and traversal and exposes ambiguous cursor flags', async () => {
  const continuation = token => ({ continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token } } } });
  const playlist = { contents: { playlistVideoListRenderer: { contents: [continuation('one-private-token'), continuation('two-private-token')] } }, continuationContents: { ignored: Array.from({ length: 16000 }, () => ({ richGridContinuation: {} })) } };
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  const shape = result.result.responseShape;
  assert.equal(shape.counts.continuationContents.richGridContinuation, 10000);
  assert.equal(shape.truncated, true);
  assert.equal(shape.ambiguous, true);
  assert.equal(shape.recognizedCursor, false);
  assert.equal(JSON.stringify(shape).includes('private-token'), false);
  for (const bucket of Object.values(shape.counts)) for (const count of Object.values(bucket)) assert.ok(Number.isInteger(count) && count >= 0 && count <= 10000);
});

test('Continuation response headers cannot replace the initial global playlist total', async () => {
  const first = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist: playlistPageFixtures.initial }));
  const playlist = { header: { playlistHeaderRenderer: { numVideosText: { simpleText: '5 videos' } } }, onResponseReceivedActions: playlistPageFixtures.append.onResponseReceivedActions };
  const next = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true, cursor: first.result.nextCursor } }), fetchFixture({ playlist }));
  assert.equal(next.ok, true);
  assert.equal(next.result.responseShape.continuation, true);
  assert.equal(next.result.total, 4);
  assert.equal(next.result.playlist.total, 4);
  assert.equal(next.result.complete, true);
});


test('Ordinary playlist jobs omit opt-in structural diagnostics', async () => {
  for (const flag of [undefined, false, 'true', 1]) {
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: flag } }), fetchFixture({ playlist: playlistPageFixtures.initial }));
    assert.equal(result.ok, true);
    assert.equal(Object.prototype.hasOwnProperty.call(result.result, 'responseShape'), false);
  }
});

test('Requested-playlist append unwraps item section contents and preserves occurrences with scoped paging', async () => {
  const rows = clonePlaylistFixture(playlistPageFixtures.append.onResponseReceivedActions[0].appendContinuationItemsAction.continuationItems);
  rows.unshift(clonePlaylistFixture(rows[0]));
  const recommended = { playlistVideoRenderer: { videoId: 'recommend01', setVideoId: 'not-member', style: 'PLAYLIST_VIDEO_RENDERER_STYLE_RECOMMENDED_VIDEO' } };
  const continuation = { continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'nested-next' } } } };
  const playlist = { onResponseReceivedActions: [{ appendContinuationItemsAction: { target: 'VLPLfixture123', continuationItems: [{ itemSectionRenderer: { header: { playlistVideoRenderer: { videoId: 'headerwrong', setVideoId: 'not-member-header' } }, contents: [...rows, { itemSectionRenderer: { contents: [recommended] } }, continuation] } }] } }] };
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  assert.deepEqual(Array.from(result.result.entries, row => row.entryId), ['entry-3', 'entry-3', 'entry-deleted']);
  assert.equal(result.result.entries[2].available, false);
  assert.equal(JSON.parse(decodeURIComponent(result.result.nextCursor.slice('yt-playlist-v2:'.length))).token, 'nested-next');
  assert.equal(result.result.complete, false);
  playlist.onResponseReceivedActions[0].appendContinuationItemsAction.target = 'recommendations';
  const unrelated = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
  assert.equal(unrelated.result.entries.length, 0);
  assert.equal(unrelated.result.nextCursor, null);
});

test('Nested playlist append cursor conflicts and excessive wrappers remain ambiguous', async () => {
  const cursor = token => ({ continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token } } } });
  const wrap = contents => ({ itemSectionRenderer: { contents } });
  let tooDeep = cursor('hidden-next');
  for (let i = 0; i < 10; i++) tooDeep = wrap([tooDeep]);
  for (const items of [[wrap([cursor('one'), wrap([cursor('two')])])], [tooDeep]]) {
    const playlist = { onResponseReceivedActions: [{ appendContinuationItemsAction: { target: 'playlist-video-list', continuationItems: items } }] };
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
    assert.equal(result.ok, true);
    assert.equal(result.result.nextCursor, null);
    assert.equal(result.result.complete, false);
    assert.equal(result.result.responseShape.ambiguous, true);
  }
});

test('Repeated nested append cursor stops reviewed mutation before dispatch', async () => {
  const initial = clonePlaylistFixture(playlistPageFixtures.initial);
  initial.header.musicEditablePlaylistDetailHeaderRenderer = { playlistId: 'PLfixture123', ownerId: ACCOUNT_ID };
  const page = { onResponseReceivedActions: [{ appendContinuationItemsAction: { target: 'playlist-video-list', continuationItems: [{ itemSectionRenderer: { contents: [{ continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'playlist-next' } } } }] } }] } }] };
  const fixture = fetchFixture(); const original = fixture.fetchImpl;
  let reads = 0, edits = 0;
  fixture.fetchImpl = async (url, options) => {
    if (new URL(url).pathname === '/youtubei/v1/browse/edit_playlist') { edits++; return response({ status: 'STATUS_SUCCEEDED' }); }
    if (new URL(url).pathname === '/youtubei/v1/browse') { reads++; return response(JSON.parse(options.body).continuation ? page : initial); }
    return original(url, options);
  };
  const result = await executeYouTubeJob(job({ operation: 'applyPlaylistEdits', args: { previewApproved: true, playlistId: 'PLfixture123', expectedEntries: [], actions: [{ type: 'add', trackId: 'abcdefghijk' }] } }), fixture);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'PROVIDER_CHANGED');
  assert.equal(reads, 2);
  assert.equal(edits, 0);
});

test('Initial playlist browse and reviewed edit preflight request unavailable occurrences', async () => {
  const fixture = fetchFixture({ playlist: playlistPageFixtures.initial });
  const read = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fixture);
  assert.equal(read.ok, true);
  const body = fixture.calls.find(call => new URL(call.url).pathname === '/youtubei/v1/browse').body;
  assert.deepEqual(Object.keys(body).sort(), ['browseId', 'context', 'params']);
  assert.equal(body.browseId, 'VLPLfixture123');
  assert.equal(body.params, 'wgYCCAA=');

  const editable = { header: { musicEditablePlaylistDetailHeaderRenderer: { playlistId: 'PLfixture123', ownerId: ACCOUNT_ID }, playlistHeaderRenderer: { numVideosText: { simpleText: '0 videos' } } }, contents: { playlistVideoListRenderer: { contents: [] } } };
  const editFixture = fetchFixture({ playlist: editable });
  const original = editFixture.fetchImpl;
  editFixture.fetchImpl = async (url, options) => new URL(url).pathname === '/youtubei/v1/browse/edit_playlist' ? response({ status: 'STATUS_SUCCEEDED', playlistEditResults: [{ playlistEditVideoAddedResultData: { videoId: 'abcdefghijk', setVideoId: 'new-occurrence' } }] }) : original(url, options);
  const edited = await executeYouTubeJob(job({ operation: 'applyPlaylistEdits', args: { previewApproved: true, playlistId: 'PLfixture123', expectedEntries: [], actions: [{ type: 'add', trackId: 'abcdefghijk' }] } }), editFixture);
  assert.equal(edited.ok, true, JSON.stringify(edited));
  const preflight = editFixture.calls.find(call => new URL(call.url).pathname === '/youtubei/v1/browse').body;
  assert.deepEqual(Object.keys(preflight).sort(), ['browseId', 'context', 'params']);
  assert.equal(preflight.browseId, 'VLPLfixture123');
  assert.equal(preflight.params, 'wgYCCAA=');
});

test('Inspection distinguishes membership styles and source-rule cursor ancestry without token disclosure', async () => {
  const cursor = token => ({ continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token } } } });
  const ordinary = { playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: 'entry-one', isPlayable: true } };
  const recommended = { playlistVideoRenderer: { videoId: 'recommend01', isPlayable: true, style: 'PLAYLIST_VIDEO_RENDERER_STYLE_RECOMMENDED_VIDEO' } };
  const unavailable = { playlistVideoRenderer: { setVideoId: 'deleted-one', isPlayable: false } };
  const playlist = { contents: { sectionListRenderer: { contents: [cursor('outer-private-token'), { itemSectionRenderer: { contents: [{ playlistVideoListRenderer: { contents: [ordinary, unavailable, recommended, cursor('member-private-token')] } }] } }] } } };
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  const shape = result.result.responseShape;
  assert.deepEqual(JSON.parse(JSON.stringify(shape.membership)), { allResponse: { ordinaryRows: 2, recommendedRows: 1, unavailableRows: 1, rowsWithOccurrenceId: 2 }, recognizedSections: { ordinaryRows: 2, recommendedRows: 1, unavailableRows: 1, rowsWithOccurrenceId: 2 } });
  assert.equal(shape.cursorLocations.namedListDirect, 1);
  assert.equal(shape.cursorLocations.sectionListDirect, 1);
  assert.equal(shape.retainedRuleCandidatePresent, true);
  assert.equal(shape.retainedRuleEqualsSelected, true);
  assert.equal(JSON.stringify(shape).includes('private-token'), false);
  assert.equal(JSON.stringify(shape).includes('entry-one'), false);
});

test('Inspection exposes source-rule divergence across mixed nested section cursors', async () => {
  const cursor = token => ({ continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token } } } });
  const playlist = { contents: { sectionListRenderer: { contents: [{ itemSectionRenderer: { contents: [cursor('other-first')] } }, { playlistVideoListRenderer: { contents: [cursor('named-selected')] } }] } } };
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  assert.equal(result.result.responseShape.cursorLocations.sectionListOtherItemSection, 1);
  assert.equal(result.result.responseShape.cursorLocations.namedListDirect, 1);
  assert.equal(result.result.responseShape.retainedRuleCandidatePresent, true);
  assert.equal(result.result.responseShape.retainedRuleEqualsSelected, false);
});

test('Wrapped named playlist continuation wins over direct section recommendation cursor', async () => {
  const continuationCommand = { token: 'member-next', request: 'CONTINUATION_REQUEST_TYPE_BROWSE' };
  for (const endpoint of [{ innertubeCommand: { continuationCommand } }, { command: { continuationCommand } }, { performOnceCommand: { continuationCommand } }, { commandExecutorCommand: { commands: [{ innertubeCommand: { continuationCommand } }] } }, { serialCommand: { commands: [{ parallelCommand: { commands: [{ continuationCommand }] } }] } }]) {
    const playlist = { contents: { sectionListRenderer: { contents: [{ playlistVideoListRenderer: { contents: [{ continuationItemRenderer: { continuationEndpoint: endpoint } }] } }, { continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'recommendations-next' } } } }] } } };
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
    assert.equal(result.ok, true);
    assert.equal(JSON.parse(decodeURIComponent(result.result.nextCursor.slice('yt-playlist-v2:'.length))).token, 'member-next');
    assert.equal(result.result.responseShape.retainedRuleEqualsSelected, true);
  }
});

test('Unsupported or conflicting wrapped cursor commands cannot fall back to recommendations', async () => {
  for (const endpoint of [{ futureCommand: { token: 'unsupported' } }, { commandExecutorCommand: { commands: [{ continuationCommand: { token: 'one' } }, { continuationCommand: { token: 'two' } }] } }, { commandExecutorCommand: { commands: [{ continuationCommand: { token: 'search-token', request: 'CONTINUATION_REQUEST_TYPE_SEARCH' } }] } }]) {
    const playlist = { contents: { sectionListRenderer: { contents: [{ playlistVideoListRenderer: { contents: [{ continuationItemRenderer: { continuationEndpoint: endpoint } }] } }, { continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'outer' } } } }] } } };
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
    assert.equal(result.ok, true);
    assert.equal(result.result.nextCursor, null);
    assert.equal(result.result.responseShape.ambiguous, true);
  }
});

test('Three-page playlist preserves initial total and completes only at exact cumulative terminal count', async () => {
  const row = i => ({ playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: `occurrence-${i}` } });
  const cursor = token => ({ continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token } } } });
  for (const [known, lastCount, expectedComplete] of [[true, 40, true], [true, 39, false], [false, 40, false]]) {
    const pages = [
      { ...(known ? { header: { playlistHeaderRenderer: { numVideosText: { simpleText: '240 videos' } } } } : {}), contents: { playlistVideoListRenderer: { contents: [...Array.from({ length: 100 }, (_, i) => row(i)), cursor('page-two')] } } },
      { onResponseReceivedActions: [{ appendContinuationItemsAction: { target: 'playlist-video-list', continuationItems: [...Array.from({ length: 100 }, (_, i) => row(100 + i)), cursor('page-three')] } }] },
      { header: { playlistHeaderRenderer: { numVideosText: { simpleText: '5 videos' } } }, onResponseReceivedActions: [{ appendContinuationItemsAction: { target: 'playlist-video-list', continuationItems: Array.from({ length: lastCount }, (_, i) => row(200 + i)) } }] }
    ];
    const fixture = fetchFixture(); const original = fixture.fetchImpl; let index = 0;
    fixture.fetchImpl = async (url, options) => new URL(url).pathname === '/youtubei/v1/browse' ? response(pages[index++]) : original(url, options);
    let token;
    for (let page = 0; page < 3; page++) {
      const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', ...(token ? { cursor: token } : {}) } }), fixture);
      assert.equal(result.ok, true, JSON.stringify(result));
      assert.equal(result.result.total, known ? 240 : null);
      assert.equal(result.result.playlist.total, known ? 240 : null);
      assert.equal(result.result.complete, page === 2 ? expectedComplete : false);
      token = result.result.nextCursor;
      if (page < 2) {
        assert.match(token, /^yt-playlist-v2:/);
        const state = JSON.parse(decodeURIComponent(token.slice('yt-playlist-v2:'.length)));
        assert.equal(state.offset, (page + 1) * 100);
        assert.equal(state.expectedTotal, known ? 240 : null);
      } else assert.equal(token, null);
    }
  }
});

test('Empty playlist identity diagnostics require exact target-bound owner and selected privacy evidence', async () => {
  const playlist = {
    microformat: { microformatDataRenderer: { canonicalUrl: 'https://www.youtube.com/playlist?list=PLfixture123' } },
    sidebar: { playlistSidebarRenderer: {
      playlistSidebarPrimaryInfoRenderer: {
        playlistId: 'VLPLfixture123',
        navigationEndpoint: { watchEndpoint: { playlistId: 'PLfixture123' } },
        stats: [{ runs: [{ text: 'No videos' }] }],
        privacyForm: { dropdownFormFieldRenderer: { dropdown: { dropdownRenderer: { entries: [
          { privacyDropdownItemRenderer: { isSelected: false, icon: { iconType: 'PRIVACY_PUBLIC' } } },
          { privacyDropdownItemRenderer: { isSelected: true, icon: { iconType: 'PRIVACY_PRIVATE' } } }
        ] } } } }
      },
      playlistSidebarSecondaryInfoRenderer: { videoOwner: { videoOwnerRenderer: { title: { runs: [{ navigationEndpoint: { browseEndpoint: { browseId: ACCOUNT_ID } } }] } } } }
    } }
  };
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  const shape = result.result.responseShape;
  assert.equal(shape.identityEvidence.primaryPlaylistIds, 1);
  assert.equal(shape.identityEvidence.primaryNavigationPlaylistIds, 1);
  assert.equal(shape.identityEvidence.canonicalPlaylistIds, 1);
  assert.equal(shape.identityEvidence.privacyDropdownSelectedPrivate, 1);
  assert.equal(shape.identityEvidence.sidebarVideoOwnerMatchesConfirmedAccount, 1);
  assert.equal(shape.totalEvidence.sidebarFirstStatEnglishNoVideos, 1);
  assert.equal(shape.totalEvidence.exactTotalIsZero, false, 'sidebar count alone must not stand in for exact page total');
  assert.equal(JSON.stringify(shape).includes('PLfixture123'), false);
  assert.equal(JSON.stringify(shape).includes(ACCOUNT_ID), false);

  const invalid = {
    microformat: { microformatDataRenderer: { canonicalUrl: 'https://evil.example/playlist?list=PLfixture123' } },
    sidebar: { playlistSidebarRenderer: { playlistSidebarPrimaryInfoRenderer: {
      playlistId: 'PLdifferent123', navigationEndpoint: { watchEndpoint: { playlistId: 'PLdifferent123' } },
      stats: [{ simpleText: 'No videos' }],
      privacyForm: { dropdownFormFieldRenderer: { dropdown: { dropdownRenderer: { entries: [
        { privacyDropdownItemRenderer: { isSelected: 'true', icon: { iconType: 'PRIVACY_PRIVATE' } } }
      ] } } } }
    }, playlistSidebarSecondaryInfoRenderer: { videoOwner: { videoOwnerRenderer: { title: { runs: [{ navigationEndpoint: { browseEndpoint: { browseId: 'UCforeign000000000000000000' } } }] } } } } } }
  };
  const rejected = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist: invalid }));
  assert.equal(rejected.ok, true);
  assert.equal(rejected.result.responseShape.identityEvidence.primaryPlaylistIds, 0);
  assert.equal(rejected.result.responseShape.identityEvidence.primaryNavigationPlaylistIds, 0);
  assert.equal(rejected.result.responseShape.identityEvidence.canonicalPlaylistIds, 0);
  assert.equal(rejected.result.responseShape.identityEvidence.privacyDropdownSelectedPrivate, 0);
  assert.equal(rejected.result.responseShape.identityEvidence.sidebarVideoOwnerMismatches, 1);
});

test('Playlist v2 rejects invalid cumulative counts before browse; v1 stays read-only incomplete', async () => {
  const state = { accountId: ACCOUNT_ID, playlistId: 'PLfixture123', origin: 'https://www.youtube.com', token: 'terminal', offset: 100, expectedTotal: 240 };
  for (const change of [{ offset: -1 }, { offset: 1.5 }, { offset: Number.MAX_SAFE_INTEGER + 1 }, { offset: 241 }, { expectedTotal: -1 }, { expectedTotal: '240' }, { accountId: 'other' }, { playlistId: 'PLother12345' }, { offset: undefined }]) {
    const fixture = fetchFixture({ playlist: playlistPageFixtures.append });
    const cursor = `yt-playlist-v2:${encodeURIComponent(JSON.stringify({ ...state, ...change }))}`;
    const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', cursor } }), fixture);
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'UNSUPPORTED');
    assert.equal(fixture.calls.some(call => new URL(call.url).pathname === '/youtubei/v1/browse'), false);
  }
  const { offset, expectedTotal, ...legacy } = state;
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', cursor: `yt-playlist-v1:${encodeURIComponent(JSON.stringify(legacy))}` } }), fetchFixture({ playlist: playlistPageFixtures.append }));
  assert.equal(result.ok, true);
  assert.equal(result.result.total, null);
  assert.equal(result.result.complete, false);
});


test('Include-unavailable playlist read retains unavailable occurrence IDs and exact count', async () => {
  const playlist = { header: { playlistHeaderRenderer: { numVideosText: { simpleText: '3 videos' } } }, contents: { playlistVideoListRenderer: { contents: [
    { playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: 'available-one', isPlayable: true, title: { simpleText: 'Fixture' } } },
    { playlistVideoRenderer: { setVideoId: 'private-occurrence', isPlayable: false, title: { simpleText: '[Private video]' }, unplayableText: { simpleText: 'Video unavailable' } } },
    { playlistVideoRenderer: { setVideoId: 'deleted-occurrence', isPlayable: false, title: { simpleText: '[Deleted video]' } } }
  ] } } };
  const fixture = fetchFixture({ playlist });
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fixture);
  assert.equal(result.ok, true);
  assert.equal(fixture.calls.find(call => new URL(call.url).pathname === '/youtubei/v1/browse').body.params, 'wgYCCAA=');
  assert.deepEqual(Array.from(result.result.entries, row => row.entryId), ['available-one', 'private-occurrence', 'deleted-occurrence']);
  assert.deepEqual(Array.from(result.result.entries, row => row.trackId), ['abcdefghijk', null, null]);
  assert.deepEqual(Array.from(result.result.entries, row => row.available), [true, false, false]);
  assert.equal(result.result.total, 3);
  assert.equal(result.result.complete, true);
});

test('Opt-in playability counters distinguish explicit flags and exact placeholder labels without changing availability', async () => {
  const values = [
    { isPlayable: true, title: { simpleText: 'Ordinary' }, navigationEndpoint: { watchEndpoint: { videoId: 'abcdefghijk' } } },
    { isPlayable: false, title: { simpleText: '[Private video]' } },
    { title: { runs: [{ text: 'Deleted video' }] } },
    { isPlayable: null, title: { simpleText: '[dElEtEd ViDeO]' } },
    { isPlayable: 'true', title: { simpleText: 'Private video remix' } },
    { title: { simpleText: '[Private video' } },
    { title: { simpleText: 'Deleted videos' } }
  ];
  const playlist = { contents: { playlistVideoListRenderer: { contents: values.map((row, index) => ({ playlistVideoRenderer: { videoId: 'abcdefghijk', setVideoId: `occ-${index}`, ...row } })) } } };
  const run = inspectResponseShape => executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape } }), fetchFixture({ playlist }));
  const result = await run(true);
  assert.equal(result.ok, true);
  const expected = { explicitTrue: 1, explicitFalse: 1, missing: 3, invalid: 2, withWatchEndpoint: 1, knownPlaceholder: 3 };
  assert.deepEqual(JSON.parse(JSON.stringify(result.result.responseShape.playability)), { allResponse: expected, recognizedSections: expected });
  const ordinary = await run(false);
  assert.equal(ordinary.ok, true);
  assert.equal(Object.prototype.hasOwnProperty.call(ordinary.result, 'responseShape'), false);
  assert.deepEqual(Array.from(result.result.entries, row => row.available), Array.from(ordinary.result.entries, row => row.available));
  assert.equal(result.result.entries[2].available, false, 'missing flags do not confirm playability');
});


test('Playlist availability requires explicit true flag rather than placeholder title or watch endpoint', async () => {
  const rows = [
    { setVideoId: 'missing-private', title: { simpleText: '[Private video]' } },
    { setVideoId: 'missing-ordinary', title: { simpleText: 'Ordinary legacy title' } },
    { setVideoId: 'true-private-title', title: { simpleText: 'Private video' }, isPlayable: true },
    { setVideoId: 'false-ordinary', title: { simpleText: 'Ordinary' }, isPlayable: false },
    { setVideoId: 'true-unplayable', isPlayable: true, unplayableText: { simpleText: 'Unavailable' } },
    { setVideoId: 'invalid-flag', isPlayable: 'true' },
    { setVideoId: 'invalid-id', isPlayable: true, videoId: 'bad' }
  ];
  const playlist = { header: { playlistHeaderRenderer: { numVideosText: { simpleText: '7 videos' } } }, contents: { playlistVideoListRenderer: { contents: rows.map(row => ({ playlistVideoRenderer: { videoId: 'abcdefghijk', navigationEndpoint: { watchEndpoint: { videoId: 'abcdefghijk' } }, ...row } })) } } };
  const result = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123', inspectResponseShape: true } }), fetchFixture({ playlist }));
  assert.equal(result.ok, true);
  assert.equal(result.result.entries.length, 7);
  assert.deepEqual(Array.from(result.result.entries, row => row.entryId), rows.map(row => row.setVideoId));
  assert.deepEqual(Array.from(result.result.entries, row => row.available), [false, false, true, false, false, false, false]);
  assert.equal(result.result.total, 7);
  assert.equal(result.result.complete, true);
  assert.equal(result.result.responseShape.membership.recognizedSections.unavailableRows, 6);
  assert.equal(result.result.responseShape.playability.recognizedSections.withWatchEndpoint, 7);
});

const indexedLockupPlaylist = (count = 62) => ({
  header: { playlistHeaderRenderer: { title: { simpleText: 'Public indexed playlist' }, numVideosText: { simpleText: `${count} videos` } } },
  contents: { twoColumnBrowseResultsRenderer: { tabs: [{ tabRenderer: { selected: true, content: { sectionListRenderer: { contents: [
    { itemSectionRenderer: { contents: Array.from({ length: count }, (_, index) => ({ lockupViewModel: {
      contentType: 'LOCKUP_CONTENT_TYPE_VIDEO', contentId: 'abcdefghijk',
      metadata: { lockupMetadataViewModel: { title: { content: `Occurrence ${index}` } } },
      rendererContext: { commandContext: { onTap: { innertubeCommand: { watchEndpoint: { videoId: 'abcdefghijk', playlistId: 'PLfixture123', index } } } } },
    } })) } },
    { continuationItemViewModel: { continuationCommand: { continuationCommand: { token: 'unrelated-recommendations', request: 'CONTINUATION_REQUEST_TYPE_BROWSE' } } } },
  ] } } } }] } },
});

test('initial selected WEB indexed lockups preserve all 62 occurrences without using recommendation cursor', async () => {
  const r = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist: indexedLockupPlaylist() }));
  assert.equal(r.ok, true);
  assert.equal(r.result.entries.length, 62);
  assert.equal(r.result.total, 62);
  assert.equal(r.result.complete, true);
  assert.equal(r.result.nextCursor, null);
  assert.equal(r.result.entries[61].title, 'Occurrence 61');
  assert.equal(r.result.entries[0].entryId, null);
});

test('initial indexed lockups reject wrong playlist, wrong index, and unselected tabs', async () => {
  for (const mutate of [p => { p.contents.twoColumnBrowseResultsRenderer.tabs[0].tabRenderer.selected = false; }, p => { p.contents.twoColumnBrowseResultsRenderer.tabs[0].tabRenderer.content.sectionListRenderer.contents[0].itemSectionRenderer.contents[1].lockupViewModel.rendererContext.commandContext.onTap.innertubeCommand.watchEndpoint.playlistId = 'PLother12345'; }, p => { p.contents.twoColumnBrowseResultsRenderer.tabs[0].tabRenderer.content.sectionListRenderer.contents[0].itemSectionRenderer.contents[1].lockupViewModel.rendererContext.commandContext.onTap.innertubeCommand.watchEndpoint.index = 0; }]) {
    const playlist = indexedLockupPlaylist(2); mutate(playlist);
    const r = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
    assert.equal(r.result.entries.length, 0);
    assert.equal(r.result.complete, false);
    assert.equal(r.result.nextCursor, null);
  }
});

test('WEB playlist lockups read supplied bottom-overlay durations and thumbnail image sources', async () => {
  const playlist = indexedLockupPlaylist(3);
  const rows = playlist.contents.twoColumnBrowseResultsRenderer.tabs[0].tabRenderer.content.sectionListRenderer.contents[0].itemSectionRenderer.contents;
  const badge = (text, badgeStyle = 'THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT') => ({ thumbnailBadgeViewModel: { text, badgeStyle } });
  rows[0].lockupViewModel.contentImage = { thumbnailViewModel: { image: { sources: [{ url: 'https://i.ytimg.com/small.jpg' }, { url: 'https://i.ytimg.com/large.jpg' }] }, overlays: [{ thumbnailBottomOverlayViewModel: { badges: [badge('2:28')] } }] } };
  rows[1].lockupViewModel.contentImage = { thumbnailViewModel: { overlays: [{ thumbnailBottomOverlayViewModel: { badges: [badge('LIVE', 'THUMBNAIL_OVERLAY_BADGE_STYLE_LIVE'), badge('2:99')] } }] } };
  rows[2].lockupViewModel.contentImage = { thumbnailViewModel: { overlays: [{ thumbnailBottomOverlayViewModel: { badges: [badge('2:28'), badge('3:01')] } }] } };
  const r = await executeYouTubeJob(job({ operation: 'getPlaylist', args: { playlistId: 'PLfixture123' } }), fetchFixture({ playlist }));
  assert.equal(r.result.entries[0].durationSeconds, 148);
  assert.equal(r.result.entries[0].artworkUrl, 'https://i.ytimg.com/large.jpg');
  assert.equal(r.result.entries[1].durationSeconds, null);
  assert.equal(r.result.entries[1].artworkUrl, null);
  assert.equal(r.result.entries[2].durationSeconds, null);
});


test('own library bounded cursor decoding distinguishes direct wrapped invalid repeated and terminal', async () => {
  const cursorItem = token => ({ continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token } } } });
  const wrapped = commands => ({ continuationItemRenderer: { continuationEndpoint: { commandExecutorCommand: { commands } } } });
  const run = async (items, args = {}) => {
    const playlist = { richGridRenderer: { contents: items }, sidebar: { continuationItemRenderer: { continuationEndpoint: { continuationCommand: { token: 'unrelated-secret' } } } } };
    return executeYouTubeJob(job({operation:'listPlaylists',args:{inspectLibraryResponseShape:true,...args}}), fetchFixture({playlist}));
  };
  for (const [entry, field] of [[cursorItem('member-secret'), 'directCursorItems'], [wrapped([{continuationCommand:{token:'member-secret'}}]), 'wrappedCursorItems']]) {
    const result = await run([entry]);
    assert.equal(result.ok,true);
    assert.ok(result.result.nextCursor);
    assert.equal(result.result.libraryResponseShape[field],1);
    assert.equal(result.result.libraryResponseShape.unrecognizedContinuationNodes,1);
    assert.equal(result.result.complete,null);
    assert.doesNotMatch(JSON.stringify(result.result.libraryResponseShape), /secret|SAPISID|unrelated/);
  }
  for (const entries of [[cursorItem('a'),cursorItem('b')], [cursorItem('same'),cursorItem('same')], [cursorItem('')], [wrapped([{continuationCommand:{token:'a'}},{continuationCommand:{token:'b'}}])]]) {
    const result = await run(entries);
    assert.equal(result.result.nextCursor,null);
    assert.equal(result.result.complete,false);
    assert.equal(result.result.truncated,true);
    assert.equal(result.result.libraryResponseShape.ambiguous,true);
  }
  const initial = await run([cursorItem('same')]);
  const repeated = await run([cursorItem('same')],{cursor:initial.result.nextCursor});
  assert.equal(repeated.result.libraryResponseShape.repeatedCursorItems,1);
  assert.equal(repeated.result.complete,false);
  const terminal = await executeYouTubeJob(job({operation:'listPlaylists',args:{inspectLibraryResponseShape:true}}),fetchFixture({playlist:{richGridRenderer:{contents:[]}}}));
  assert.equal(terminal.result.libraryResponseShape.terminalObserved,true);
  assert.equal(terminal.result.complete,null); // terminal structure is not full-library coverage proof.
  const unknown = await executeYouTubeJob(job({operation:'listPlaylists',args:{inspectLibraryResponseShape:true}}),fetchFixture({playlist:{unknown:{contents:[]}}}));
  assert.equal(unknown.result.libraryResponseShape.terminalObserved,false);
  const ordinary = await executeYouTubeJob(job({operation:'listPlaylists'}),fetchFixture({playlist:{richGridRenderer:{contents:[]}}}));
  assert.equal(Object.hasOwn(ordinary.result,'libraryResponseShape'),false);
});

test('own library explicit continuation containers and append commands retain scoped cursors', async () => {
 const entry = {continuationItemRenderer:{continuationEndpoint:{commandExecutorCommand:{commands:[{continuationCommand:{token:'owned-next'}}]}}}};
 for (const playlist of [
   {continuationContents:{gridContinuation:{items:[entry]}}},
   {gridRenderer:{items:[],continuations:[{nextContinuationData:{continuation:'owned-next'}}]}},
   {onResponseReceivedActions:[{commandExecutorCommand:{commands:[{appendContinuationItemsAction:{continuationItems:[entry]}}]}}]},
 ]) {
   const result = await executeYouTubeJob(job({operation:'listPlaylists',args:{inspectLibraryResponseShape:true}}),fetchFixture({playlist}));
   assert.equal(result.ok,true);
   assert.ok(result.result.nextCursor);
   assert.equal(result.result.libraryResponseShape.distinctTokens,1);
 }
 const unsupported = {onResponseReceivedActions:[{appendContinuationItemsAction:{target:'recommendations',continuationItems:[entry]}}]};
 const result = await executeYouTubeJob(job({operation:'listPlaylists',args:{inspectLibraryResponseShape:true}}),fetchFixture({playlist:unsupported}));
 assert.equal(result.result.nextCursor,null);
 assert.equal(result.result.libraryResponseShape.unrecognizedContinuationNodes,1);
 assert.equal(result.result.libraryResponseShape.terminalObserved,false);
});

test('YouTube creation receipt marks rejected account as explicitly not dispatched', async () => {
  const fixture=fetchFixture();
  const result=await executeYouTubeJob(job({creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args:{accountId:'UCbbbbbbbbbbbbbbbbbbbbbb'}}),fixture);
  assert.equal(result.ok,false);assert.equal(result.error.creationReceipt.mutationDispatched,false);
  assert.equal(result.error.creationReceipt.playlistId,null);assert.match(result.error.creationReceipt.requestId,/^[a-f0-9]{32}$/);
  assert.doesNotMatch(JSON.stringify(result.error.creationReceipt),/UCbbbb|cookie|SAPISID|https?:/i);
  assert.equal(fixture.calls.some(call=>new URL(call.url).pathname==='/youtubei/v1/playlist/create'),false);
});
test('YouTube creation receipt preserves postdispatch timeout uncertainty', async () => {
  const fixture=fetchFixture(),base=fixture.fetchImpl;let writes=0;
  fixture.fetchImpl=async(url,options)=>{if(new URL(url).pathname==='/youtubei/v1/playlist/create'){writes++;throw new DOMException('private timeout detail','AbortError');}return base(url,options);};
  const result=await executeYouTubeJob(job({creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args:{accountId:ACCOUNT_ID}}),fixture);
  assert.equal(result.error.code,'OUTCOME_UNKNOWN');assert.equal(result.error.creationReceipt.mutationDispatched,true);
  assert.equal(result.error.creationReceipt.playlistId,null);assert.equal(writes,1);assert.doesNotMatch(JSON.stringify(result),/private timeout detail/);
});
test('YouTube successful creation receipt validates returned playlist ID', async () => {
  for(const valid of [true,false]){
    const fixture=fetchFixture(),base=fixture.fetchImpl;
    fixture.fetchImpl=async(url,options)=>new URL(url).pathname==='/youtubei/v1/playlist/create'?response({playlistId:valid?CREATED_VERIFICATION_ID:'bad'}):base(url,options);
    const result=await executeYouTubeJob(job({creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args:{accountId:ACCOUNT_ID}}),fixture);
    const receipt=result.ok?result.result.creationReceipt:result.error.creationReceipt;
    assert.equal(result.ok,valid);assert.equal(receipt.mutationDispatched,true);assert.equal(receipt.playlistId,valid?CREATED_VERIFICATION_ID:null);
  }
});

test('creation rejects a missing or malformed native request ID before any network dispatch', async () => {
  for (const requestId of [undefined, 'private-cookie-value', 'ABCDEF0123456789ABCDEF0123456789']) {
    let calls=0;
    const input=job({operation:'createVerificationPlaylist',args:{accountId:ACCOUNT_ID}});
    if(requestId!==undefined) input.creationRequestId=requestId;
    const result=await executeYouTubeJob(input,{fetchImpl:async()=>{calls++;throw new Error('must not dispatch');}});
    assert.equal(result.ok,false);assert.equal(calls,0);assert.equal(result.error.creationReceipt,undefined);
    assert.doesNotMatch(JSON.stringify(result),/private-cookie-value/);
  }
});

test('reviewed YouTube UUID intent creates exactly reconstructed private empty title without description mutation',async()=>{
 const intent='123e4567-e89b-42d3-a456-426614174000',title=`Burrata verification ${intent}`,fixture=fetchFixture(),base=fixture.fetchImpl;let writes=0;
 fixture.fetchImpl=async(url,options)=>{if(new URL(url).pathname!=='/youtubei/v1/playlist/create')return base(url,options);writes++;const{context,...body}=JSON.parse(options.body);assert.deepEqual(body,{title,privacyStatus:'PRIVATE',videoIds:[]});return response({playlistId:CREATED_VERIFICATION_ID});};
 const args={accountId:ACCOUNT_ID,verificationIntentId:intent};
 const result=await executeYouTubeJob(job({creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args}),fixture);assert.equal(result.ok,true,JSON.stringify(result));assert.equal(result.result.title,title);assert.equal(result.result.creationReceipt.mutationDispatched,true);assert.equal(writes,1);
 for(const bad of [{...args,title:'arbitrary'},{...args,description:'hidden'},{...args,verificationIntentId:'../bad'}]){const failed=await executeYouTubeJob(job({creationRequestId:'0123456789abcdef0123456789abcdef',operation:'createVerificationPlaylist',args:bad}),fixture);assert.equal(failed.ok,false);assert.equal(writes,1);}
});

test('modern empty page binds canonical and playlist edit commands when primary navigation is absent', async () => {
  const id='PLfixture123';
  const fixture=()=>({
    header:{pageHeaderRenderer:{content:{pageHeaderViewModel:{actions:[{playlistEditEndpoint:{playlistId:id,actions:[]}}]}}}},
    microformat:{microformatDataRenderer:{urlCanonical:`https://www.youtube.com/playlist?list=${id}`}},
    contents:{twoColumnBrowseResultsRenderer:{tabs:[{tabRenderer:{selected:true,content:{sectionListRenderer:{contents:[{itemSectionRenderer:{contents:[]}}]}}}}]}},
    sidebar:{playlistSidebarRenderer:{items:[
      {playlistSidebarPrimaryInfoRenderer:{menu:{playlistEditEndpoint:{playlistId:id,actions:[]}},stats:[{simpleText:'No videos'}],privacyForm:{dropdownFormFieldRenderer:{dropdown:{dropdownRenderer:{entries:[{privacyDropdownItemRenderer:{isSelected:true,icon:{iconType:'PRIVACY_PRIVATE'}}}]}}}}}},
      {playlistSidebarSecondaryInfoRenderer:{videoOwner:{videoOwnerRenderer:{title:{runs:[{navigationEndpoint:{browseEndpoint:{browseId:ACCOUNT_ID}}}]}}}}}
    ]}}
  });
  const read=async playlist=>{const response=await executeYouTubeJob(job({operation:'getPlaylist',args:{playlistId:id,inspectResponseShape:true}}),fetchFixture({playlist}));assert.equal(response.ok,true);return response.result;};
  const valid=await read(fixture());
  assert.equal(valid.complete,true);
  assert.equal(valid.total,0);
  assert.equal(valid.playlist.ownerId,ACCOUNT_ID);
  assert.equal(valid.playlist.visibility,'private');
  assert.equal(valid.playlist.canEdit,false);
  assert.equal(valid.responseShape.identityEvidence.canonicalPlaylistIds,1);
  assert.equal(valid.responseShape.identityEvidence.editEndpointPlaylistIds,2);
  assert.equal(valid.responseShape.identityEvidence.modernMatchingIds,2);
  assert.equal(valid.responseShape.identityEvidence.modernConflictingIds,0);
  assert.equal(valid.responseShape.identityEvidence.modernTargetBound,1);
  assert.equal(valid.responseShape.identityEvidence.modernMatchingOwners,1);
  assert.equal(valid.responseShape.identityEvidence.modernIdentityRecognized,1);
  assert.equal(valid.responseShape.identityEvidence.modernEmptyRecognized,1);
  const navigationOnly=fixture();delete navigationOnly.header;delete navigationOnly.microformat;delete navigationOnly.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.menu;navigationOnly.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.navigationEndpoint={browseEndpoint:{browseId:`VL${id}`}};assert.equal((await read(navigationOnly)).complete,true);
  const conflict=fixture();conflict.header.pageHeaderRenderer.content.pageHeaderViewModel.actions[0].playlistEditEndpoint.playlistId='PLforeign-secret';const conflictResult=await read(conflict);assert.equal(conflictResult.responseShape.identityEvidence.modernConflictingIds,1);assert.equal(conflictResult.responseShape.identityEvidence.modernTargetBound,0);assert.equal(JSON.stringify(conflictResult).includes('PLforeign-secret'),false);
  const missing=fixture();delete missing.header.pageHeaderRenderer.content.pageHeaderViewModel.actions[0].playlistEditEndpoint.playlistId;assert.equal((await read(missing)).responseShape.identityEvidence.modernMalformedEditIds,1);
  for(const [canonical,reason,protocol,host,path,count,matches] of [
    [7,1,0,0,0,0,0],['invalid-private-url',2,0,0,0,0,0],
    [`https://user:pass@www.youtube.com/playlist?list=${id}`,3,1,1,1,1,1],
    [`ftp://www.youtube.com/playlist?list=${id}`,4,3,1,1,1,1],
    [`https://youtube.com/playlist?list=${id}`,5,1,2,1,1,1],
    [`https://music.youtube.com/playlist?list=${id}`,5,1,3,1,1,1],
    [`https://www.youtube.com:8443/playlist?list=${id}`,6,1,1,1,1,1],
    [`https://www.youtube.com/watch?list=${id}`,7,1,1,2,1,1],
    [`https://www.youtube.com/playlist?list=${id}&list=${id}`,8,1,1,1,2,1],
    ['https://www.youtube.com/playlist?list=PLforeign-secret',9,1,1,1,1,0],
  ]){const f=fixture();f.microformat.microformatDataRenderer.urlCanonical=canonical;const result=await read(f);const e=result.responseShape.identityEvidence;assert.equal(result.complete,false);assert.equal(e.canonicalRejectReason,reason);assert.equal(e.canonicalProtocolClass,protocol);assert.equal(e.canonicalHostClass,host);assert.equal(e.canonicalPathClass,path);assert.equal(e.canonicalListCount,count);assert.equal(e.canonicalListMatches,matches);assert.equal(JSON.stringify(result).includes('PLforeign-secret'),false);}
  const http=fixture();http.microformat.microformatDataRenderer.urlCanonical=`http://www.youtube.com/playlist?list=${id}`;
  http.header.pageHeaderRenderer.content.pageHeaderViewModel.actions=Array.from({length:6},()=>({playlistEditEndpoint:{playlistId:id,actions:[]}}));
  http.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.menu={commands:Array.from({length:7},()=>({playlistEditEndpoint:{playlistId:id,actions:[]}}))};
  const httpTransport=fetchFixture({playlist:http});
  const httpResponse=await executeYouTubeJob(job({operation:'getPlaylist',args:{playlistId:id,inspectResponseShape:true}}),httpTransport);
  assert.equal(httpResponse.ok,true);assert.equal(httpResponse.result.complete,true);
  assert.equal(httpResponse.result.playlist.ownerId,ACCOUNT_ID);assert.equal(httpResponse.result.playlist.visibility,'private');assert.equal(httpResponse.result.playlist.canEdit,false);
  assert.equal(httpResponse.result.responseShape.identityEvidence.canonicalRejectReason,0);
  assert.equal(httpResponse.result.responseShape.identityEvidence.canonicalProtocolClass,2);
  assert.equal(httpResponse.result.responseShape.identityEvidence.modernMatchingIds,13);
  assert.equal(httpResponse.result.responseShape.identityEvidence.modernRejectedCanonicals,0);
  assert.equal(httpTransport.calls.every(call=>new URL(call.url).protocol==='https:'),true);
  assert.equal(httpTransport.calls.some(call=>new URL(call.url).pathname.endsWith('/edit_playlist')),false);
  for(const mutate of [
    f=>f.header.pageHeaderRenderer.content.pageHeaderViewModel.actions[0].playlistEditEndpoint.playlistId='PLother',
    f=>f.microformat.microformatDataRenderer.urlCanonical='http://www.youtube.com/playlist?list=PLother',
    f=>f.microformat.microformatDataRenderer.urlCanonical=`http://youtube.com/playlist?list=${id}`,
    f=>f.microformat.microformatDataRenderer.urlCanonical=`http://www.youtube.com:8080/playlist?list=${id}`,
    f=>f.microformat.microformatDataRenderer.urlCanonical=`http://user:pass@www.youtube.com/playlist?list=${id}`,
  ]){const bad=structuredClone(http);mutate(bad);assert.equal((await read(bad)).complete,false);}
  const capability=fixture();
  capability.header.pageHeaderRenderer.content.pageHeaderViewModel.actions=[
    {commandMetadata:{webCommandMetadata:{apiUrl:'/youtubei/v1/browse/edit_playlist'}},playlistEditEndpoint:{playlistId:id,params:'private-params-marker',actions:[{action:'ACTION_ADD_VIDEO',addedVideoId:'private-video-marker'},{action:'ACTION_REMOVE_VIDEO'},{action:'ACTION_MOVE_VIDEO_BEFORE'},{action:'ACTION_SET_PLAYLIST_PRIVACY'},{action:'ACTION_SET_PLAYLIST_NAME'},{action:'UNKNOWN-private-action-marker'},null]}},
    {playlistEditEndpoint:{playlistId:'PLforeign-secret',actions:[{action:'ACTION_ADD_VIDEO'}]}},
    {playlistEditEndpoint:{actions:[]}},
  ];
  capability.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.menu={playlistEditEndpoint:{playlistId:id}};
  capability.contents.extra={playlistVideoListRenderer:{playlistId:id,isEditable:true,canReorder:true}};
  const capabilityResult=await read(capability);const editEvidence=capabilityResult.responseShape.identityEvidence;
  assert.equal(capabilityResult.playlist.canEdit,false,'diagnostic edit evidence does not grant permissions');
  for(const [key,value] of Object.entries({editMatchedCommands:2,editForeignCommands:1,editMalformedCommands:1,editApiPathMatched:1,editParamsPresent:1,editActionsMissing:1,editActionsMalformed:1,editActionAdd:1,editActionRemove:1,editActionMove:1,editActionPrivacy:1,editActionMetadata:1,editActionOther:1,editFlagTrue:1,editReorderTrue:1}))assert.equal(editEvidence[key],value,key);
  for(const marker of ['private-params-marker','private-video-marker','UNKNOWN-private-action-marker','PLforeign-secret'])assert.equal(JSON.stringify(capabilityResult).includes(marker),false);
  for(const region of ['header','sidebar','framework']){
    const page=fixture();page[region]??={};page[region].unrelatedContinuationFramework={};assert.equal((await read(page)).complete,true);
  }
  const memberContinuation=fixture();memberContinuation.contents.twoColumnBrowseResultsRenderer.tabs[0].tabRenderer.content.unrelatedContinuationFramework={};const memberContinuationResult=await read(memberContinuation);assert.equal(memberContinuationResult.complete,false);assert.equal(memberContinuationResult.responseShape.identityEvidence.emptyRejectReason,9);assert.equal(memberContinuationResult.responseShape.identityEvidence.emptyContinuationContents,1);
  const deep=(leaf={})=>{let root={},cursor=root;for(let i=0;i<50;i++){cursor.child={};cursor=cursor.child;}Object.assign(cursor,leaf);return root;};
  const irrelevant=fixture();irrelevant.frameworkUpdates=deep();irrelevant.responseContext={payload:Array.from({length:10001},()=>({counter:1}))};assert.equal((await read(irrelevant)).complete,true);
  const deepMember=fixture();deepMember.contents.twoColumnBrowseResultsRenderer.tabs[0].tabRenderer.content.sectionListRenderer.contents[0].itemSectionRenderer.contents=[{messageRenderer:{text:{simpleText:'No videos'},payload:deep({playlistVideoRenderer:{videoId:'abcdefghijk'},continuationItemRenderer:{}})}}];const deepMemberResult=await read(deepMember);assert.equal(deepMemberResult.complete,false);assert.equal(deepMemberResult.responseShape.identityEvidence.emptyRejectReason,8);
  for(const key of ['onResponseReceivedActions','onResponseReceivedEndpoints','onResponseReceivedCommands']){
    const page=fixture();page[key]=[{commandExecutorCommand:{commands:[{appendContinuationItemsAction:{target:id,continuationItems:[{playlistVideoRenderer:{videoId:'abcdefghijk'}}]}}]}}];assert.equal((await read(page)).complete,false);
    const tooDeep=fixture();tooDeep[key]=[deep()];assert.equal((await read(tooDeep)).responseShape.identityEvidence.emptyRejectReason,8);
  }
  const alternateContinuation=fixture();alternateContinuation.continuationContents={unknownContainer:{}};assert.equal((await read(alternateContinuation)).responseShape.identityEvidence.emptyRejectReason,9);
  const spacedZero=fixture();spacedZero.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.stats=[{runs:[{text:'No\u00a0videos'}]}];assert.equal((await read(spacedZero)).total,0);assert.equal((await read(spacedZero)).responseShape.identityEvidence.emptyRejectReason,0);assert.equal((await read(spacedZero)).complete,true);
  const countConflict=fixture();countConflict.metadata={playlistMetadataRenderer:{videoCount:0}};countConflict.header.playlistHeaderRenderer={playlistId:id,numVideosText:{simpleText:'1 video'}};assert.equal((await read(countConflict)).responseShape.identityEvidence.emptyRejectReason,5);
  const headerConflict=fixture();headerConflict.header.playlistHeaderRenderer={playlistId:id,privacy:'PUBLIC'};assert.equal((await read(headerConflict)).responseShape.identityEvidence.emptyRejectReason,6);
  for(const phrase of ['No\u00a0videos','Ｎｏ ｖｉｄｅｏｓ',' No  \t videos ']){const page=fixture();page.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.stats=[{runs:[{text:phrase}]}];const normalized=await read(page);assert.equal(normalized.complete,true);assert.equal(normalized.total,0);}
  for(const phrase of ['No videos extra','No videos2','No video','No views']){const page=fixture();page.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.stats=[{runs:[{text:phrase}]}];assert.equal((await read(page)).complete,false);}
  const positiveFullwidth=fixture();positiveFullwidth.metadata={playlistMetadataRenderer:{videoCount:0}};positiveFullwidth.header.playlistHeaderRenderer={playlistId:id,numVideosText:{simpleText:'１ video'}};assert.equal((await read(positiveFullwidth)).complete,false);
  const otherKinds=fixture();otherKinds.header.pageHeaderRenderer.content.pageHeaderViewModel.actions=[{playlistEditEndpoint:{playlistId:id,actions:[{action:'ACTION_ADD_PLAYLIST'},{action:'ACTION_JOIN_COLLABORATION'},{action:'ACTION_CREATE_COLLABORATION_INVITE_LINK'},{action:'ACTION_SET_CLOSED_TO_CONTRIBUTIONS'},{action:'ACTION_SET_ALLOW_ITEM_VOTE'}]}}];const classifiedOther=await read(otherKinds);assert.equal(classifiedOther.responseShape.identityEvidence.editActionCollection,1);assert.equal(classifiedOther.responseShape.identityEvidence.editActionCollaboration,3);assert.equal(classifiedOther.responseShape.identityEvidence.editActionVoting,1);assert.equal(classifiedOther.playlist.canEdit,false);
  const eligible=()=>{
    const page=fixture();
    page.header.pageHeaderRenderer.content.pageHeaderViewModel.actions=[{commandMetadata:{webCommandMetadata:{apiUrl:'/youtubei/v1/browse/edit_playlist'}},playlistEditEndpoint:{playlistId:id,actions:[{action:'ACTION_SET_PLAYLIST_NAME'}]}}];
    page.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.menu={commandMetadata:{webCommandMetadata:{apiUrl:'/youtubei/v1/browse/edit_playlist'}},playlistEditEndpoint:{playlistId:id,actions:[{action:'ACTION_SET_PLAYLIST_PRIVACY'}]}};
    return page;
  };
  const eligibleRead=await read(eligible());assert.equal(eligibleRead.playlist.canEdit,false);assert.deepEqual(JSON.parse(JSON.stringify(eligibleRead.playlist.writeCapabilities)),{add:true,remove:false,reorder:false});
  assert.deepEqual(JSON.parse(JSON.stringify((await read(fixture())).playlist.writeCapabilities)),{add:false,remove:false,reorder:false},'ownership and empty rows alone do not grant additions');
  const applyOn=async(page,actions,{approved=true,loseResult=false}={})=>{
    const wire=fetchFixture({playlist:page});const base=wire.fetchImpl;let writes=0;
    wire.fetchImpl=async(url,options)=>{if(new URL(url).pathname.endsWith('/edit_playlist')){writes++;if(loseResult)throw Error('fixture-lost-result');return response({status:'STATUS_SUCCEEDED',playlistEditResults:[{playlistEditVideoAddedResultData:{videoId:'abcdefghijk',setVideoId:'new-owned-occurrence'}}]});}return base(url,options);};
    const result=await executeYouTubeJob(job({operation:'applyPlaylistEdits',args:{previewApproved:approved,playlistId:id,expectedEntries:[],actions}}),wire);return {result,writes};
  };
  const oneAdd=[{type:'add',trackId:'abcdefghijk'}];
  const approvedAdd=await applyOn(eligible(),oneAdd);assert.equal(approvedAdd.result.ok,true);assert.equal(approvedAdd.writes,1);
  const lostAdd=await applyOn(eligible(),oneAdd,{loseResult:true});assert.equal(lostAdd.result.error.code,'OUTCOME_UNKNOWN');assert.equal(lostAdd.writes,1,'a lost result is never retried');
  assert.equal((await applyOn(eligible(),oneAdd,{approved:false})).writes,0);
  assert.equal((await applyOn(fixture(),oneAdd)).writes,0);
  for(const action of [{type:'remove',entryId:'some-occurrence',trackId:'abcdefghijk'},{type:'moveBefore',entryId:'some-occurrence',trackId:'abcdefghijk',successorEntryId:null}]){const denied=await applyOn(eligible(),[action]);assert.equal(denied.result.ok,false);assert.equal(denied.writes,0);}
  const changedOwner=eligible();changedOwner.sidebar.playlistSidebarRenderer.items[1].playlistSidebarSecondaryInfoRenderer.videoOwner.videoOwnerRenderer.title.runs[0].navigationEndpoint.browseEndpoint.browseId='UCbbbbbbbbbbbbbbbbbbbbbb';assert.equal((await applyOn(changedOwner,oneAdd)).writes,0);
  const editOnly=fixture();delete editOnly.microformat;assert.equal((await read(editOnly)).complete,true);
  const canonicalOnly=fixture();delete canonicalOnly.header;delete canonicalOnly.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.menu;assert.equal((await read(canonicalOnly)).complete,true);
  for(const mutate of [
    f=>f.microformat.microformatDataRenderer.urlCanonical='https://www.youtube.com/playlist?list=PLother',
    f=>f.microformat.microformatDataRenderer.urlCanonical=`https://user:pass@www.youtube.com/playlist?list=${id}`,
    f=>f.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.title={runs:{}},
    ...['toString','__proto__'].map(icon=>f=>f.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.privacyForm.dropdownFormFieldRenderer.dropdown.dropdownRenderer.entries[0].privacyDropdownItemRenderer.icon.iconType=icon),
    f=>f.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.navigationEndpoint={browseEndpoint:{browseId:'VLPLother'}},
    f=>f.header.pageHeaderRenderer.content.pageHeaderViewModel.actions[0].playlistEditEndpoint.playlistId='PLother',
    f=>f.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.menu.playlistEditEndpoint.playlistId='PLother',
    f=>delete f.header.pageHeaderRenderer.content.pageHeaderViewModel.actions[0].playlistEditEndpoint.playlistId,
    f=>{delete f.microformat;delete f.header;delete f.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.menu;},
    f=>f.metadata={playlistMetadataRenderer:{videoCount:1}},
    f=>f.contents.twoColumnBrowseResultsRenderer.tabs[0].tabRenderer.content.sectionListRenderer.contents[0].itemSectionRenderer.contents.push({playlistVideoRenderer:{videoId:'abcdefghijk',setVideoId:'occurrence'}}),
    f=>f.contents.twoColumnBrowseResultsRenderer.tabs[0].tabRenderer.content.sectionListRenderer.contents[0].itemSectionRenderer.contents.push({continuationItemRenderer:{}}),
    f=>f.sidebar.playlistSidebarRenderer.items.push(f.sidebar.playlistSidebarRenderer.items[0]),
    f=>f.sidebar.playlistSidebarRenderer.items[0].playlistSidebarPrimaryInfoRenderer.privacyForm.dropdownFormFieldRenderer.dropdown.dropdownRenderer.entries[0].privacyDropdownItemRenderer.icon.iconType='UNKNOWN',
    f=>f.sidebar.playlistSidebarRenderer.items[1].playlistSidebarSecondaryInfoRenderer.videoOwner.videoOwnerRenderer.title.runs[0].navigationEndpoint.browseEndpoint.browseId='UCbbbbbbbbbbbbbbbbbbbbbb',
    f=>f.contents.twoColumnBrowseResultsRenderer.tabs.push(f.contents.twoColumnBrowseResultsRenderer.tabs[0]),
  ]){const changed=fixture();mutate(changed);assert.equal((await read(changed)).complete,false);}
});
