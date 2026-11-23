<script lang="ts">
  import type { SubsonicStatus } from "./lib/subsonic";

  let { call, status, onStatus }: {
    call: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
    status: SubsonicStatus | null;
    onStatus: (status: SubsonicStatus) => void;
  } = $props();

  let endpoint = $state("");
  let username = $state("");
  let password = $state("");
  let error = $state("");
  let busy = $state("");
  let editing = $state(false);

  async function run(label: string, command: string, args?: Record<string, unknown>) {
    if (busy) return;
    error = "";
    busy = label;
    try {
      const next = await call<SubsonicStatus | null>(command, args);
      if (next) onStatus(next);
      if (command === "connect_subsonic") editing = false;
    } catch (failure) {
      error = String(failure);
    } finally {
      busy = "";
    }
  }

  async function connect(event: SubmitEvent) {
    event.preventDefault();
    const currentPassword = password;
    password = "";
    await run("Connecting…", "connect_subsonic", { endpoint: endpoint.trim(), username: username.trim(), password: currentPassword });
  }

  function changePreferences(visible: boolean, allowFallback: boolean) {
    void run("Saving settings…", "set_subsonic_preferences", { visible, allowFallback });
  }
</script>

<section class="subsonic-settings" aria-labelledby="subsonic-heading">
  <h3 id="subsonic-heading">Subsonic</h3>
  {#if status?.endpoint && !editing}
    <p>{status.username} · {status.endpoint}</p>
    <div class="preferences">
      <label><input type="checkbox" checked={status.visible} disabled={Boolean(busy)}
        onchange={event => changePreferences(event.currentTarget.checked, status!.allowFallback)} /> Show Subsonic in Library</label>
      <label><input type="checkbox" checked={status.allowFallback} disabled={Boolean(busy)}
        onchange={event => changePreferences(status!.visible, event.currentTarget.checked)} /> Try verified alternate recordings when the server is unavailable</label>
    </div>
    <p>Hiding Subsonic keeps its tracks in your Burrata playlists and Liked Songs.</p>
    <div class="actions">
      <button type="button" disabled={Boolean(busy)} onclick={() => void run("Refreshing catalog…", "refresh_subsonic")}>Refresh catalog</button>
      {#if status.canSeed}
        {#if status.strawberryFound}<button type="button" disabled={Boolean(busy)} onclick={() => void run("Checking Strawberry cache…", "seed_subsonic", { discovered: true })}>Use Strawberry cache</button>{/if}
        <button type="button" disabled={Boolean(busy)} onclick={() => void run("Checking Strawberry cache…", "seed_subsonic", { discovered: false })}>Choose Strawberry database…</button>
      {/if}
      <button type="button" disabled={Boolean(busy)} onclick={() => {
        endpoint = status!.endpoint ?? ""; username = status!.username ?? ""; editing = true;
      }}>Edit connection</button>
      <button type="button" disabled={Boolean(busy)} onclick={() => void run("Disconnecting…", "disconnect_subsonic")}>Disconnect</button>
    </div>
    {#if status.trackCount > 0}<p>{status.trackCount.toLocaleString()} tracks in the Subsonic catalog</p>{/if}
  {:else}
    <form onsubmit={connect}>
      <label>Server URL <input type="url" bind:value={endpoint} required placeholder="https://music.example.com" /></label>
      <label>Username <input bind:value={username} required autocomplete="username" /></label>
      <label>Password <input type="password" bind:value={password} required autocomplete="current-password" /></label>
      <div class="actions">
        {#if editing}<button type="button" onclick={() => { editing = false; password = ""; }} disabled={Boolean(busy)}>Cancel</button>{/if}
        <button class="primary" type="submit" disabled={Boolean(busy)}>Connect</button>
      </div>
    </form>
  {/if}
  {#if busy}<p role="status">{busy}</p>{/if}
  {#if status?.warning}<p role="status">{status.warning}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</section>

<style>
  .subsonic-settings { display: grid; gap: .65rem; padding: 1rem; border: 1px solid var(--line); border-radius: 10px; }
  .subsonic-settings h3, .subsonic-settings p { margin: 0; }
  .subsonic-settings p { overflow-wrap: anywhere; }
  .subsonic-settings form, .preferences { display: grid; gap: .5rem; }
  .subsonic-settings form > label { display: grid; gap: .25rem; }
  .actions { display: flex; flex-wrap: wrap; gap: .5rem; }
</style>
