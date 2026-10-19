import { performance } from 'node:perf_hooks';

export function failure(code, stage) {
  return Object.assign(new Error(code), { code, benchmarkStage: stage });
}

export function commandBudget(budgetMs, { signal, started = performance.now() } = {}) {
  if (!Number.isInteger(budgetMs) || budgetMs < 1 || budgetMs > 120_000) throw failure('INVALID_BUDGET');
  const controller = new AbortController();
  const abort = () => controller.abort(signal?.reason ?? failure('CANCELLED'));
  if (signal?.aborted) abort();
  else signal?.addEventListener('abort', abort, { once: true });
  const remaining = Math.max(0, budgetMs - (performance.now() - started));
  const timer = setTimeout(() => controller.abort(failure('TIMEOUT')), remaining);
  if (!remaining) controller.abort(failure('TIMEOUT'));
  return {
    signal: controller.signal,
    async run(operation) {
      if (performance.now() - started >= budgetMs) controller.abort(failure('TIMEOUT'));
      controller.signal.throwIfAborted();
      let listener;
      try {
        const result = await Promise.race([
          Promise.resolve().then(operation),
          new Promise((_, reject) => {
            listener = () => reject(controller.signal.reason);
            controller.signal.addEventListener('abort', listener, { once: true });
          }),
        ]);
        if (performance.now() - started >= budgetMs) controller.abort(failure('TIMEOUT'));
        controller.signal.throwIfAborted();
        return result;
      } finally { controller.signal.removeEventListener('abort', listener); }
    },
    close() { clearTimeout(timer); signal?.removeEventListener('abort', abort); },
  };
}
