import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('./background.js', import.meta.url), 'utf8');
const flush = () => new Promise(resolve => setTimeout(resolve, 0));

function harness({ tabs = [{ id: 7, url: 'https://monochrome.samidy.com/' }], nativeFailure = null } = {}) {
  const calls = [];
  const stored = [];
  const messages = [];
  const order = [];
  let interval;
  let captureStopped = 0;
  let report = { schemaVersion: 1, responses: [{ kind: 'resolver', response: { title: 'Fixture' } }] };
  const api = {
    permissions: { contains: async query => { calls.push(['permissions.contains', query]); return true; } },
    tabs: { query: async query => { calls.push(['tabs.query', query]); return tabs; } },
    scripting: { executeScript: async options => {
      calls.push(['executeScript', options]);
      if (options.func) {
        order.push(options.func.toString().includes('window[key]?.stop') ? 'reset' : 'read');
        return [{ result: options.func() }];
      } else if (options.files) {
        order.push('inject');
        context.__burrataMonochromeResearchV1 = { stop: () => { captureStopped++; }, read: () => report };
      }
      return [];
    } },
    storage: { local: { set: async value => { stored.push(value); } } },
    runtime: { sendNativeMessage: async (_host, message) => {
      messages.push(message);
      if (nativeFailure) throw nativeFailure;
      return message.report ? { ok: true } : { ok: true };
    } },
  };
  const context = vm.createContext({ browser: api, console: { info: (...args) => calls.push(['console.info', ...args]) },
    Date, JSON, Map, Set, Promise, setInterval: fn => { interval = fn; return 1; }, setTimeout: () => 2, clearInterval() {} });
  context.window = context;
  context.location = { origin: 'https://monochrome.samidy.com' };
  context.__burrataMonochromeResearchV1 = { stop: () => { captureStopped++; } };
  vm.runInContext(source, context);
  return { calls, stored, messages, order, runAgain: async () => { await interval(); await flush(); }, stopped: () => captureStopped,
    setReport: value => { report = value; } };
}

test('restart resets prior page capture before reinjection and read; forwards a report once', async () => {
  const h = harness();
  await flush();
  assert.deepEqual(h.order.slice(0, 3), ['reset', 'inject', 'read']);
  const reports = h.messages.filter(message => message.report);
  assert.equal(reports.length, 1);
  assert.deepEqual(reports[0].report, { schemaVersion: 1, responses: [{ kind: 'resolver', response: { title: 'Fixture' } }] });
  await h.runAgain();
  assert.equal(h.messages.filter(message => message.report).length, 1, 'unchanged report is deduplicated');
});

test('missing tab records a fixed diagnostic and does not attempt injection or native report delivery', async () => {
  const h = harness({ tabs: [] });
  await flush();
  assert.equal(h.calls.some(call => call[0] === 'executeScript'), false);
  assert.equal(h.messages.some(message => message.report), false);
  assert.ok(h.stored.some(value => value.monochromeResearchDiagnostic?.code === 'waiting_for_monochrome_tab'));
});

test('native host failures stay best effort and do not recurse through diagnostics', async () => {
  const h = harness({ nativeFailure: Object.assign(new Error('private native failure detail'), { name: 'NativeMessagingError' }) });
  await flush();
  assert.equal(h.messages.filter(message => message.report).length, 1);
  assert.ok(h.stored.some(value => value.monochromeResearchDiagnostic?.code === 'native_host_unavailable'));
  assert.equal(h.calls.filter(call => call[0] === 'console.info').some(call => call.includes('private native failure detail')), false);
  const count = h.messages.length;
  await h.runAgain();
  assert.ok(h.messages.length <= count + 2, 'one failed report send and one deduplicated diagnostic at most');
});
