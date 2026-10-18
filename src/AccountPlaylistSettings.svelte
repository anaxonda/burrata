<script lang="ts">
  type Visibility = { spotify?: boolean; youtube?: boolean };
  let { value = {}, onChange }: {
    value?: Visibility;
    onChange: (value: { spotify: boolean; youtube: boolean }) => void;
  } = $props();

  function update(provider: "spotify" | "youtube", enabled: boolean) {
    onChange({
      spotify: provider === "spotify" ? enabled : Boolean(value.spotify),
      youtube: provider === "youtube" ? enabled : Boolean(value.youtube),
    });
  }
</script>

<section class="account-playlist-settings" aria-labelledby="account-playlist-settings-title">
  <header>
    <h3 id="account-playlist-settings-title">Playlists in the sidebar</h3>
    <p>Show playlists from connected accounts in the sidebar. Search remains available separately.</p>
  </header>
  <label>
    <input type="checkbox" checked={Boolean(value.spotify)} onchange={event => update("spotify", event.currentTarget.checked)} />
    <span>Show Spotify playlists</span>
  </label>
  <label>
    <input type="checkbox" checked={Boolean(value.youtube)} onchange={event => update("youtube", event.currentTarget.checked)} />
    <span>Show YouTube playlists</span>
  </label>
</section>

<style>
  .account-playlist-settings { display:grid; gap:9px; }
  header { display:grid; gap:4px; }
  h3 { margin:0; font-size:14px; }
  p { margin:0; color:var(--muted); font-size:12px; }
  label { display:flex; align-items:center; gap:8px; }
  input { width:16px; height:16px; }
</style>
