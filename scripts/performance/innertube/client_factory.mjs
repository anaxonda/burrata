// Diagnostic-only split of the public youtubei.js session/player APIs.
// The caller owns tokens, decoding, the absolute deadline and the process group.
import { performance } from 'node:perf_hooks';
import { failure } from './budget.mjs';

function preparationKey(client) {
  const context = client.session?.context;
  const data = context?.client;
  if (!data?.visitorData) throw failure('TOKEN_BINDING_MISSING', 'session');
  // This sensitive key remains in memory; it is never part of a report.
  return JSON.stringify([data.clientName, data.clientVersion, data.visitorData,
    data.remoteHost ?? '', client.session.cookie ?? '', client.session.account_index ?? 0]);
}

function abortable(promise, signal) {
  return new Promise((resolve, reject) => {
    const abort = () => reject(signal.reason ?? failure('CANCELLED', 'session'));
    if (signal.aborted) { abort(); return; }
    signal.addEventListener('abort', abort, { once: true });
    Promise.resolve(promise).then(value => {
      signal.removeEventListener('abort', abort);
      resolve(value);
    }, error => {
      signal.removeEventListener('abort', abort);
      reject(error);
    });
  });
}

export function createAnonymousClientFactory({
  Innertube, Player, cache, fetch, sessionOptions = {}, parallelPlayer = false,
  now = () => performance.now(),
}) {
  if (sessionOptions.cookie || sessionOptions.on_behalf_of_user || sessionOptions.account_index) {
    throw failure('INVALID_PLAYBACK_REQUEST', 'session');
  }
  return async function createClient({ signal } = {}) {
    signal?.throwIfAborted();
    const controller = new AbortController();
    const abortFromCaller = () => controller.abort(signal.reason ?? failure('CANCELLED', 'session'));
    signal?.addEventListener('abort', abortFromCaller, { once: true });
    let preparing = true;
    const started = now();
    const metrics = { mode: parallelPlayer ? 'parallel-player' : 'sequential' };
    const cleanup = () => signal?.removeEventListener('abort', abortFromCaller);
    const guard = () => controller.signal.throwIfAborted();
    const decorate = (client, ready) => {
      client.playbackPreparation = metrics;
      client.playbackReady = ready;
      client.playbackPreparationSignal = controller.signal;
      // The token branch can cancel player preparation without acquiring any
      // token values or mutating the caller's command-wide controller.
      client.abortPlaybackPreparation = reason => {
        controller.abort(reason ?? failure('CANCELLED', 'session'));
        cleanup();
      };
      return client;
    };
    // During preparation all branches share its cancellation. After readiness,
    // the caller's fetch owns subsequent request signals (including warm use).
    const preparationFetch = (input, init = {}) => {
      if (!preparing) return fetch(input, init);
      guard();
      const joined = init.signal ? AbortSignal.any([init.signal, controller.signal]) : controller.signal;
      return fetch(input, { ...init, signal: joined });
    };
    const watch = task => {
      task.catch(error => controller.abort(error));
      return task;
    };
    const timed = (name, operation) => watch((async () => {
      const start = now();
      try { return await operation(); }
      finally { metrics[name] = now() - start; }
    })());
    const options = { ...sessionOptions, cache, fetch: preparationFetch };
    if (!parallelPlayer) {
      try {
        const client = await abortable(timed('sdkCreateMs', () => Innertube.create(options)), controller.signal);
        guard();
        preparing = false;
        metrics.readyMs = now() - started;
        cleanup();
        return decorate(client, Promise.resolve(client));
      } catch (error) {
        controller.abort(error);
        cleanup();
        throw error;
      }
    }

    let capturedSession;
    let capturedKey;
    const contextTask = timed('sessionContextMs', async () => {
      const client = await Innertube.create({ ...options, retrieve_player: false });
      guard();
      capturedSession = client.session;
      capturedKey = preparationKey(client);
      return client;
    });
    const playerTask = timed('playerMs', () => Player.create(cache, preparationFetch,
      undefined, sessionOptions.player_id));
    const ready = abortable(Promise.all([contextTask, playerTask]), controller.signal).then(([client, player]) => {
      guard();
      if (client.session !== capturedSession || preparationKey(client) !== capturedKey) {
        throw failure('CANCELLED', 'session');
      }
      client.session.player = player;
      preparing = false;
      metrics.readyMs = now() - started;
      cleanup();
      return client;
    }).catch(error => {
      controller.abort(error);
      cleanup();
      throw error;
    });
    // Context may be returned before the caller attaches its join handler.
    // Observe failure immediately; the original promise still rejects to await.
    ready.catch(() => {});
    try {
      const client = await abortable(contextTask, controller.signal);
      guard();
      return decorate(client, ready);
    } catch (error) {
      controller.abort(error);
      cleanup();
      throw error;
    }
  };
}
