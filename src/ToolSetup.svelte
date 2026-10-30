<script lang="ts">
  import type { Dependency, Settings } from "./lib/types";
  import { call, desktop } from "./lib/api";

  type Platform = "windows" | "macos" | "linux";
  type ToolPath = "ytDlpPath" | "ffmpegPath" | "ffprobePath" | "jsRuntimePath";

  let {
    settings,
    dependencies,
    onChange,
    onCheck,
    onSave,
    checking = false,
  }: {
    settings: Settings;
    dependencies: Dependency[];
    onChange: (settings: Settings) => void;
    onCheck: () => void;
    onSave: () => void;
    checking?: boolean;
  } = $props();

  function detectedPlatform(): Platform {
    const agent = typeof navigator === "undefined" ? "" : navigator.userAgent;
    if (/windows/i.test(agent)) return "windows";
    if (/macintosh|mac os/i.test(agent)) return "macos";
    return "linux";
  }

  let platform = $state<Platform>(detectedPlatform());
  let linkError = $state("");
  async function openGuide(event: MouseEvent) {
    if (!desktop()) return;
    event.preventDefault();
    const url = (event.currentTarget as HTMLAnchorElement).href;
    try { await call("open_external_url", { url }); }
    catch (error) { linkError = String(error); }
  }

  const pathFields: { key: ToolPath; label: string; hint: string }[] = [
    { key: "ytDlpPath", label: "yt-dlp executable", hint: "yt-dlp or a full path to yt-dlp.exe / yt-dlp" },
    { key: "ffmpegPath", label: "FFmpeg executable", hint: "ffmpeg or a full path to the executable" },
    { key: "ffprobePath", label: "ffprobe executable", hint: "ffprobe or a full path to the executable" },
    { key: "jsRuntimePath", label: "JavaScript runtime executable", hint: "node, deno, or a full path" },
  ];

  const links: Record<string, Partial<Record<Platform, string>>> = {
    "yt-dlp": {
      windows: "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
      macos: "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos",
      linux: "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux",
    },
    "FFmpeg": {
      windows: "https://ffmpeg.org/download.html",
      macos: "https://ffmpeg.org/download.html",
      linux: "https://ffmpeg.org/download.html",
    },
    ffprobe: {
      windows: "https://ffmpeg.org/download.html",
      macos: "https://ffmpeg.org/download.html",
      linux: "https://ffmpeg.org/download.html",
    },
    "JavaScript runtime": {
      windows: "https://deno.com/",
      macos: "https://deno.com/",
      linux: "https://deno.com/",
    },
  };

  const guides: Record<string, string> = {
    "yt-dlp": "https://github.com/yt-dlp/yt-dlp/wiki/Installation",
    "FFmpeg": "https://ffmpeg.org/download.html",
    ffprobe: "https://ffmpeg.org/download.html",
    "JavaScript runtime": "https://deno.com/",
  };

  function toolName(key:ToolPath){return key==="ytDlpPath"?"yt-dlp":key==="ffmpegPath"?"ffmpeg":key==="ffprobePath"?"ffprobe":"node";}
  function toolMode(key:ToolPath){const value=settings[key];return value.startsWith("system:")?"system":value.startsWith("bundled:")||value===toolName(key)?"bundled":"custom";}
  function changeMode(key:ToolPath,mode:string){onChange({...settings,[key]:mode==="custom"?"":`${mode}:${toolName(key)}`});}
  function dependency(name: string): Dependency | undefined {
    return dependencies.find((item) => item.name.toLowerCase() === name.toLowerCase());
  }

  function isUnknown(item: Dependency | undefined): boolean {
    return Boolean(item?.message.trim().toLowerCase().startsWith("unknown:"));
  }

  function statusLabel(item: Dependency | undefined): string {
    if (!item) return "Not checked";
    if (isUnknown(item)) return "Unknown";
    return item.available ? "Available" : "Not found";
  }

  function summaryText(): string {
    const required = [
      { name: "yt-dlp", capability: "online audio" },
      { name: "FFmpeg", capability: "audio conversion" },
      { name: "ffprobe", capability: "audio file information" },
    ];
    const missing = required.filter(({ name }) => {
      const item = dependency(name);
      return item && !isUnknown(item) && !item.available;
    }).map(({ capability }) => capability);
    if (missing.length) return `Attention needed: ${missing.join(", ")} ${missing.length === 1 ? "is" : "are"} unavailable.`;
    if (!dependencies.length) return "Tools have not been checked yet.";
    if (required.some(({ name }) => !dependency(name) || isUnknown(dependency(name)))) return "Some required tools have not been checked yet.";
    return "Required audio tools are available.";
  }

  function statusText(item: Dependency | undefined): string {
    if (!item) return "Check this tool to see whether Burrata can run it.";
    if (isUnknown(item)) return item.message.replace(/^Unknown:\s*/i, "");
    return item.available ? item.version || "Ready to use" : item.message;
  }
</script>

<section class="tool-setup" aria-labelledby="tool-setup-title">
  {#if linkError}<p role="alert">{linkError}</p>{/if}
  <div class="setup-heading">
    <div>
      <h2 id="tool-setup-title">Included tools</h2>
      <p>Burrata releases include tools for finding, converting, and checking audio. Check their availability when troubleshooting.</p>
    </div>
    <button onclick={onCheck} disabled={checking}>{checking ? "Checking tools…" : "Check tools"}</button>
  </div>

  <div class="tool-summary" aria-label="Tool availability">
    <p>{summaryText()}</p>
  </div>

  <details class="tool-details">
    <summary>Tool details and custom locations</summary>
    <label class="platform-choice">
      Show download links for
      <select bind:value={platform} aria-label="Operating system for tool links">
        <option value="windows">Windows</option>
        <option value="macos">macOS</option>
        <option value="linux">Linux</option>
      </select>
    </label>

  <div class="tool-list">
    <article class="tool-card">
      <div class="tool-title">
        <div><h3>yt-dlp</h3><p>Finds and retrieves audio from supported sources.</p></div>
        <span class:ready={dependency("yt-dlp")?.available} class="tool-status">{statusLabel(dependency("yt-dlp"))}</span>
      </div>
      <p class="status-detail">{statusText(dependency("yt-dlp"))}</p>
      <div class="tool-links">
        <a href={links["yt-dlp"][platform]} target="_blank" rel="noreferrer" onclick={openGuide}>Download yt-dlp for {platform === "macos" ? "macOS" : platform}</a>
        <a href={guides["yt-dlp"]} target="_blank" rel="noreferrer" onclick={openGuide}>Installation guide</a>
      </div>
    </article>

    <article class="tool-card">
      <div class="tool-title">
        <div><h3>FFmpeg</h3><p>Converts audio and supports audio analysis.</p></div>
        <span class:ready={dependency("ffmpeg")?.available} class="tool-status">{statusLabel(dependency("ffmpeg"))}</span>
      </div>
      <p class="status-detail">{statusText(dependency("ffmpeg"))}</p>
      <div class="tool-links">
        <a href={links.FFmpeg[platform]} target="_blank" rel="noreferrer" onclick={openGuide}>FFmpeg project page · {platform === "macos" ? "macOS" : platform} downloads</a>
      </div>
    </article>

    <article class="tool-card">
      <div class="tool-title">
        <div><h3>ffprobe</h3><p>Checks downloaded audio. Included with FFmpeg builds.</p></div>
        <span class:ready={dependency("ffprobe")?.available} class="tool-status">{statusLabel(dependency("ffprobe"))}</span>
      </div>
      <p class="status-detail">{statusText(dependency("ffprobe"))}</p>
      <div class="tool-links">
        <a href={links.ffprobe[platform]} target="_blank" rel="noreferrer" onclick={openGuide}>FFmpeg project page · ffprobe with FFmpeg</a>
      </div>
    </article>

    <article class="tool-card">
      <div class="tool-title">
        <div><h3>JavaScript runtime (optional)</h3><p>Helps yt-dlp access some YouTube content. Deno is recommended.</p></div>
        <span class:ready={dependency("JavaScript runtime")?.available} class="tool-status">{statusLabel(dependency("JavaScript runtime"))}</span>
      </div>
      <p class="status-detail">{statusText(dependency("JavaScript runtime"))}</p>
      <div class="tool-links">
        <a href={links["JavaScript runtime"][platform]} target="_blank" rel="noreferrer" onclick={openGuide}>Deno installation for {platform === "macos" ? "macOS" : platform}</a>
      </div>
    </article>
  </div>

  <details class="ejs-note"><summary>YouTube tool details</summary>
    <strong>YouTube also needs yt-dlp’s EJS scripts.</strong>
    <span>Official yt-dlp release binaries include them. Some package-manager installs need extra setup. Burrata checks the runtime executable, but can only confirm EJS support by successfully inspecting a real YouTube source.</span>
    <a href="https://github.com/yt-dlp/yt-dlp/wiki/EJS" target="_blank" rel="noreferrer" onclick={openGuide}>yt-dlp EJS setup guide</a>
  </details>

  <details class="custom-paths">
    <summary>Tool locations</summary>
    <p>Bundled tools are recommended. System tools stay under your package manager’s control. Use a custom path for a specific executable.</p>
    <div class="custom-path-grid">
      {#each pathFields as field}
        <label>{field.label}
          <select aria-label={`${field.label} source`} value={toolMode(field.key)} onchange={event=>changeMode(field.key,event.currentTarget.value)}><option value="bundled">Bundled / managed</option><option value="system">System</option><option value="custom">Custom path</option></select>
          {#if toolMode(field.key) === "custom"}<span>Enter an executable name or full path</span><input
            value={settings[field.key]}
            placeholder={field.hint}
            oninput={(event) => onChange({ ...settings, [field.key]: event.currentTarget.value })}
          />{/if}
        </label>
      {/each}
    </div>
    <div class="setup-actions"><button class="primary" onclick={onSave}>Save tool paths</button></div>
  </details>

  </details>
</section>

<style>
  .tool-setup { display: grid; gap: 14px; margin: 8px 0 28px; container-type:inline-size; }
  .setup-heading { display:flex; align-items:flex-end; justify-content:space-between; gap:16px; }
  h2 { margin: 0 0 5px; font-size: 16px; }
  h3 { margin: 0 0 5px; font-size: 14px; }
  p { color: var(--muted); font-size: 12px; line-height: 1.5; margin: 0; }
  .platform-choice, .custom-path-grid label { display:grid; gap:6px; color:var(--muted); font-size:11px; }
  .platform-choice { max-width:240px; margin:14px 0; }
  .tool-summary { min-width:0; }
  .tool-details > summary { cursor:pointer; font-size:12px; font-weight:650; }
  .tool-details > .tool-list, .tool-details > .ejs-note, .tool-details > .custom-paths { margin-top:12px; }
  .tool-list { display:grid; grid-template-columns:repeat(2, minmax(0, 1fr)); gap:10px; }
  .tool-card { min-width:0; border:1px solid var(--line); border-radius:8px; background:var(--surface); padding:14px; }
  .tool-title { display:flex; align-items:flex-start; justify-content:space-between; gap:12px; }
  .tool-title > div { min-width:0; }
  .tool-status { flex:none; border:1px solid var(--line); border-radius:999px; padding:4px 8px; color:var(--muted); font-size:10px; white-space:nowrap; }
  .tool-status.ready { color:var(--accent); border-color:color-mix(in srgb, var(--accent) 45%, var(--line)); }
  .status-detail { margin-top:9px; }
  .tool-links { display:flex; flex-wrap:wrap; gap:8px 14px; margin-top:11px; }
  a { color:var(--accent); font-size:11px; }
  a:hover { text-decoration-thickness:2px; }
  .ejs-note { display:grid; gap:5px; border-left:3px solid var(--accent); background:var(--surface); padding:11px 13px; }
  .ejs-note strong { font-size:12px; }
  .custom-paths { border:1px solid var(--line); border-radius:8px; padding:12px 14px; background:var(--surface); }
  .custom-paths summary { cursor:pointer; font-size:12px; font-weight:650; }
  .custom-paths > p { margin:9px 0 12px; }
  .custom-path-grid { display:grid; grid-template-columns:repeat(2, minmax(0, 1fr)); gap:12px; margin-bottom:12px; }
  .custom-path-grid input { min-width:0; width:100%; }
  .setup-actions { display:flex; justify-content:flex-end; gap:8px; }
  @media (max-width: 900px) { .tool-list { grid-template-columns:1fr; } }
  @media (max-width: 620px) { .setup-heading { align-items:stretch; flex-direction:column; } .custom-path-grid { grid-template-columns:1fr; } }
  /* The settings pane can be narrow while the viewport is still desktop-sized
     when the player/info panel is docked beside it. Respond to this component's
     actual width so its cards and path fields don't collapse into slivers. */
  @container (max-width: 560px) { .tool-list { grid-template-columns:1fr; } .custom-path-grid { grid-template-columns:1fr; } }
  @container (max-width: 420px) { .setup-heading { align-items:stretch; flex-direction:column; } }
</style>
