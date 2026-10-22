// Added only to the temporary research build, not the shipped companion.
(() => {
  const api = globalThis.browser || chrome;
  const previous = new Map();
  const initializedTabs = new Set();
  let busy = false;
  let lastDiagnostic = '';
  const errorCode = error => {
    const name = typeof error?.name === 'string' ? error.name : '';
    // Inspect the message only to map known native-messaging failures to a
    // fixed code; never persist or print the message itself.
    const message = typeof error?.message === 'string' ? error.message : '';
    if (/no such native application|native application .* not found/i.test(message)) return 'native_host_unregistered';
    if (name === 'NotAllowedError' || name === 'PermissionDeniedError') return 'permission_denied';
    if (name === 'NativeMessagingError') return 'native_host_unavailable';
    if (name === 'DataCloneError') return 'report_not_serializable';
    return 'api_error';
  };
  async function diagnostic(stage, code, tabId = null) {
    const key = JSON.stringify([stage, code, tabId]);
    if (key === lastDiagnostic) return;
    lastDiagnostic = key;
    const value = { at: new Date().toISOString(), stage, code, tabId };
    // Fixed codes only: no URL, page data, exception message, or report contents.
    try { await api.storage.local.set({ monochromeResearchDiagnostic: value }); } catch {}
    // A separate fixed-code message lets the local helper update its bounded
    // status file even when no report has been captured yet. Do not recurse or
    // surface helper errors from this best-effort diagnostic path.
    try { await api.runtime.sendNativeMessage('org.burrata.monochrome_research', { diagnostic: { stage, code } }); } catch {}
    console.info('[Monochrome research]', stage, code);
  }
  async function collect() {
    if (busy) return;
    busy = true;
    try {
      let tabs;
      let hasSitePermission;
      try { hasSitePermission = await api.permissions.contains({ origins: ['https://monochrome.samidy.com/*'] }); }
      catch (error) { await diagnostic('monitor', errorCode(error)); return; }
      if (!hasSitePermission) { await diagnostic('monitor', 'site_permission_missing'); return; }
      try { tabs = await api.tabs.query({ url: 'https://monochrome.samidy.com/*' }); }
      catch (error) { await diagnostic('tab_query', errorCode(error)); return; }
      if (!tabs.length) { await diagnostic('monitor', 'waiting_for_monochrome_tab'); return; }
      for (const tab of tabs) {
        try {
          if (!initializedTabs.has(tab.id)) {
            try {
              await api.scripting.executeScript({ target: { tabId: tab.id }, world: 'MAIN', func: () => {
                if (location.origin !== 'https://monochrome.samidy.com' || window !== window.top) return;
                const key = '__burrataMonochromeResearchV1';
                try { window[key]?.stop?.(); } catch {}
                try { delete window[key]; } catch {}
              } });
              initializedTabs.add(tab.id);
            } catch (error) {
              await diagnostic('capture_injection', errorCode(error), tab.id);
              continue;
            }
          }
          try {
            await api.scripting.executeScript({ target: { tabId: tab.id }, world: 'MAIN', files: ['research/capture.js'] });
          } catch (error) { await diagnostic('capture_injection', errorCode(error), tab.id); continue; }
          let results;
          try {
            results = await api.scripting.executeScript({ target: { tabId: tab.id }, world: 'MAIN', func: () => window.__burrataMonochromeResearchV1?.read() });
          } catch (error) { await diagnostic('capture_read', errorCode(error), tab.id); continue; }
          const report = results?.[0]?.result;
          if (!report) { await diagnostic('capture_read', 'empty_report', tab.id); continue; }
          const serialized = JSON.stringify(report);
          if (serialized === previous.get(tab.id)) continue;
          let result;
          try { result = await api.runtime.sendNativeMessage('org.burrata.monochrome_research', { report }); }
          catch (error) { await diagnostic('native_message', errorCode(error), tab.id); continue; }
          if (result?.ok) {
            previous.set(tab.id, serialized);
            await diagnostic('native_message', 'saved', tab.id);
          } else await diagnostic('native_message', 'host_rejected_report', tab.id);
        } catch (error) { await diagnostic('capture', errorCode(error), tab.id); }
      }
    } finally { busy = false; }
  }
  void (async () => {
    await diagnostic('monitor', 'starting');
    await collect();
    const interval = setInterval(() => { void collect(); }, 3000);
    setTimeout(() => clearInterval(interval), 6 * 60 * 1000);
  })();
})();
