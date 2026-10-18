import test from 'node:test';
import assert from 'node:assert/strict';
import { executeJob } from './provider-session-worker.mjs';
const cookieHeader = 'sp_dc=prototype-cookie';
const expiresAt = Date.now() + 120_000;
function fixture(calls, tokenBody = { accessToken: 'TEST-FIXTURE-CREDENTIAL-44B11DAA22-private-fixture-token', expiresIn: 120, isAnonymous: false }) {
  return async (raw, options = {}) => {
    const url = new URL(raw); calls.push(`${url.host}${url.pathname}`);
    if (url.hostname === 'api.github.com') return Response.json({ files: { 'registry.json': { content: JSON.stringify([{ s: 'JBSWY3DPEHPK3PXP', v: 61 }]) } } });
    if (url.pathname === '/api/server-time') return Response.json({ serverTime: 100000 });
    if (url.pathname === '/api/token') return Response.json(tokenBody);
    if (url.hostname === 'francescograzioso.github.io') return Response.json({ operations: {} });
    if (url.pathname === '/pathfinder/v2/query') {
      const req = JSON.parse(options.body);
      if (req.operationName === 'profileAttributes') return Response.json({ data: { me: { profile: { uri: 'spotify:user:fixture-account', name: 'Fixture' } } } });
      return Response.json({ data: { artistUnion: { __typename: 'NotFound' } } });
    }
    throw Error('unexpected fixture URL');
  };
}
test('private auth refresh returns bounded expiry and cached jobs still verify account', async () => {
  const refreshCalls = [];
  const refresh = await executeJob({ provider: 'spotify', operation: 'internalAuthRefresh', internalAuthRefresh: true, args: {}, cookieHeader }, { fetchImpl: fixture(refreshCalls) });
  assert.equal(refresh.ok, true, JSON.stringify({error:refresh.error,paths:refreshCalls}));
  assert.equal(refresh.internalAuth.accessToken, 'TEST-FIXTURE-CREDENTIAL-44B11DAA22-private-fixture-token');
  assert.ok(refresh.internalAuth.expiresAt > Date.now() + 30_000);
  assert.equal(refreshCalls.filter(p => p.endsWith('/api/token')).length, 1);
  const calls = [];
  const response = await executeJob({ provider: 'spotify', operation: 'status', args: {}, cookieHeader, cachedAuth: refresh.internalAuth }, { fetchImpl: fixture(calls) });
  assert.equal(response.ok, true);
  assert.equal(response.result.accountId, 'fixture-account');
  assert.equal(calls.filter(p => p.endsWith('/api/token')).length, 0);
  assert.equal(calls.some(p => p.endsWith('/pathfinder/v2/query')), true);
  assert.equal(JSON.stringify(response).includes('TEST-FIXTURE-CREDENTIAL-44B11DAA22-private-fixture-token'), false);
});
test('private auth refresh returns a non-cacheable one-job lease when expiry is missing or invalid', async () => {
  for (const body of [{ accessToken: 'TEST-FIXTURE-CREDENTIAL-44B11DAA22-private-fixture-token' }, { accessToken: 'TEST-FIXTURE-CREDENTIAL-44B11DAA22-private-fixture-token', expiresIn: -1 }, { accessToken: 'TEST-FIXTURE-CREDENTIAL-44B11DAA22-private-fixture-token', expiresIn: 1e99 }]) {
    const calls = [];
    const result = await executeJob({ provider: 'spotify', operation: 'internalAuthRefresh', internalAuthRefresh: true, args: {}, cookieHeader }, { fetchImpl: fixture(calls, body) });
    assert.equal(result.ok, true);
    assert.equal(result.internalAuth.cacheable, false);
    assert.ok(result.internalAuth.expiresAt > Date.now() + 30_000);
    const operationCalls = [];
    const operation = await executeJob({ provider: 'spotify', operation: 'status', args: {}, cookieHeader, cachedAuth: result.internalAuth }, { fetchImpl: fixture(operationCalls) });
    assert.equal(operation.ok, true);
    assert.equal(operationCalls.filter(path => path.endsWith('/api/token')).length, 0);
  }
});
