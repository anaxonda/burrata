(() => {
  const api = globalThis.browser || chrome;
  const empty = document.getElementById('empty');
  const status = document.getElementById('status');
  const grant = document.getElementById('grant');
  const hint = document.getElementById('hint');
  let permissionGranted = false;
  const allowed = {
    monitor: new Set(['starting', 'waiting_for_monochrome_tab', 'site_permission_missing', 'permission_denied', 'api_error']),
    tab_query: new Set(['permission_denied', 'api_error']),
    capture_injection: new Set(['permission_denied', 'api_error']),
    capture_read: new Set(['permission_denied', 'api_error', 'empty_report']),
    native_message: new Set(['native_host_unregistered', 'native_host_unavailable', 'permission_denied', 'api_error', 'report_not_serializable', 'saved', 'host_rejected_report']),
    capture: new Set(['permission_denied', 'api_error']),
  };
  async function refresh() {
    if (permissionGranted) return;
    try {
      const result = await api.storage.local.get('monochromeResearchDiagnostic');
      const value = result?.monochromeResearchDiagnostic;
      if (!value || !allowed[value.stage]?.has(value.code)) {
        status.hidden = true;
        empty.hidden = false;
        grant.hidden = true;
        hint.hidden = true;
        return;
      }
      document.getElementById('stage').textContent = value.stage;
      document.getElementById('code').textContent = value.code;
      empty.hidden = true;
      status.hidden = false;
      grant.hidden = !(value.stage === 'monitor' && value.code === 'site_permission_missing');
      hint.hidden = true;
    } catch {
      document.getElementById('stage').textContent = 'status unavailable';
      document.getElementById('code').textContent = '';
      empty.hidden = true;
      status.hidden = false;
      grant.hidden = true;
      hint.hidden = true;
    }
  }
  grant.addEventListener('click', async () => {
    grant.disabled = true;
    try {
      const granted = await api.permissions.request({ origins: ['https://monochrome.samidy.com/*'] });
      if (granted) {
        permissionGranted = true;
        document.getElementById('stage').textContent = 'monitor';
        document.getElementById('code').textContent = 'permission_granted_reload';
        hint.textContent = 'Reload the Monochrome page, then reopen this panel.';
        hint.hidden = false;
        grant.hidden = true;
      } else {
        hint.textContent = 'Permission was not granted.';
        hint.hidden = false;
      }
    } catch {
      hint.textContent = 'Permission request failed.';
      hint.hidden = false;
    } finally { grant.disabled = false; }
  });
  void refresh();
  setInterval(() => { void refresh(); }, 1000);
})();
