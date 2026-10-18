<script lang="ts">
  import {
    groupPlaylistsBySection,
    movePlaylistAssignment,
    PLAYLIST_SECTION_DRAG_TYPE,
    readDraggedPlaylistId,
    type PlaylistCollection,
    type PlaylistSection,
    type PlaylistSectionAssignments,
  } from "./lib/playlist-sections";

  let {
    sections,
    showUnsorted = true,
    filterActive = false,
    assignments,
    collections,
    activeCollectionId,
    onPlaylist,
    onPlaylistContext,
    onCreateSection,
    onRenameSection,
    onDeleteSection,
    onMovePlaylist,
    onCreatePlaylist,
  }: {
    sections: PlaylistSection[];
    showUnsorted?: boolean;
    filterActive?: boolean;
    assignments: PlaylistSectionAssignments;
    collections: PlaylistCollection[];
    activeCollectionId: string;
    onPlaylist: (id: string) => void;
    onPlaylistContext: (id: string, event: MouseEvent) => void;
    onCreateSection: (name: string) => void;
    onRenameSection: (id: string, name: string) => void;
    onDeleteSection: (id: string) => void;
    onMovePlaylist: (playlistId: string, sectionId: string | null) => void;
    onCreatePlaylist: (sectionId?: string) => void;
  } = $props();

  let creatingSection = $state(false);
  let sectionName = $state("");
  let renamingSectionId = $state<string | null>(null);
  let renameValue = $state("");
  let dragTarget = $state<string | null>(null);

  const groups = $derived(groupPlaylistsBySection(sections, assignments, collections).filter(group=>showUnsorted || group.sectionId!==null));
  function targetKey(sectionId: string | null) { return sectionId ?? ""; }
  function isDragTarget(sectionId: string | null) { return dragTarget === targetKey(sectionId); }
  function allowDrop(event: DragEvent, sectionId: string | null) {
    const transfer = event.dataTransfer;
    if (!transfer || !Array.from(transfer.types).includes(PLAYLIST_SECTION_DRAG_TYPE)) return;
    event.preventDefault();
    if (transfer) transfer.dropEffect = "move";
    dragTarget = targetKey(sectionId);
  }
  function drop(event: DragEvent, sectionId: string | null) {
    event.preventDefault();
    dragTarget = null;
    const transfer = event.dataTransfer;
    if (!transfer) return;
    const playlistId = readDraggedPlaylistId(transfer, collections);
    if (!playlistId) return;
    if (movePlaylistAssignment(assignments, playlistId, sectionId, sections, collections)) onMovePlaylist(playlistId, sectionId);
  }
  function startDrag(event: DragEvent, playlistId: string) {
    if (!event.dataTransfer || !collections.some((playlist) => playlist.id === playlistId)) return;
    event.dataTransfer.setData(PLAYLIST_SECTION_DRAG_TYPE, playlistId);
    event.dataTransfer.effectAllowed = "move";
  }
  function moveFromSelector(playlistId: string, value: string) {
    const sectionId = value || null;
    if (movePlaylistAssignment(assignments, playlistId, sectionId, sections, collections)) onMovePlaylist(playlistId, sectionId);
  }
  function createSection(event: SubmitEvent) {
    event.preventDefault();
    const name = sectionName.trim();
    if (!name) return;
    onCreateSection(name);
    sectionName = "";
    creatingSection = false;
  }
  function submitRename(event: SubmitEvent, sectionId: string) {
    event.preventDefault();
    const name = renameValue.trim();
    if (!name) return;
    onRenameSection(sectionId, name);
    renamingSectionId = null;
  }
  function beginRename(section: PlaylistSection) { renamingSectionId = section.id; renameValue = section.name; }
</script>

<section class="playlist-sections" aria-label="Playlist sections">
  <header class="section-toolbar">
    <button class="add-section" aria-label="Create playlist section" title="Create playlist section" onclick={() => { creatingSection = !creatingSection; sectionName = ""; }}>＋ Section</button>
  </header>
  {#if creatingSection}
    <form class="section-form" onsubmit={createSection}>
      <label>Section name<input aria-label="New section name" bind:value={sectionName} maxlength="60" /></label>
      <div><button type="submit" disabled={!sectionName.trim()}>Create section</button><button type="button" onclick={() => { creatingSection = false; sectionName = ""; }}>Cancel</button></div>
    </form>
  {/if}

  {#each groups as group (group.sectionId ?? "__unassigned__")}
    {@const section = sections.find((item) => item.id === group.sectionId)}
    <details class="playlist-section" class:drag-over={isDragTarget(group.sectionId)} open
      ondragover={(event) => allowDrop(event, group.sectionId)}
      ondragleave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) dragTarget = null; }}
      ondrop={(event) => drop(event, group.sectionId)}>
      <summary>
        <span class="section-name">{section?.name ?? "Unsorted"}</span>
        <small>{group.playlists.length}</small>
        <span class="chevron" aria-hidden="true">⌄</span>
      </summary>
      <div class="section-content">
        <div class="section-actions">
          <button aria-label={`Create playlist in ${section?.name ?? "Unsorted"}`} onclick={() => onCreatePlaylist(section?.id)}>＋ Playlist</button>
          {#if section}
            <button aria-label={`Rename section ${section.name}`} onclick={() => beginRename(section)}>Rename</button>
            <button aria-label={`Delete section ${section.name}`} onclick={() => onDeleteSection(section.id)}>Delete</button>
          {/if}
        </div>
        {#if section && renamingSectionId === section.id}
          <form class="section-form rename-form" onsubmit={(event) => submitRename(event, section.id)}>
            <label>Rename section<input aria-label={`New name for ${section.name}`} bind:value={renameValue} maxlength="60" /></label>
            <div><button type="submit" disabled={!renameValue.trim()}>Save</button><button type="button" onclick={() => { renamingSectionId = null; }}>Cancel</button></div>
          </form>
        {/if}
        {#each group.playlists as playlist (playlist.id)}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions (The row is a draggable list item with a separate keyboard move selector.) -->
          <div class="playlist-row" class:active={activeCollectionId === playlist.id} role="listitem" draggable="true"
            ondragstart={(event) => startDrag(event, playlist.id)}>
            <button class="playlist-title" onclick={() => onPlaylist(playlist.id)} oncontextmenu={(event) => { event.preventDefault(); onPlaylistContext(playlist.id, event); }}>
              <span aria-hidden="true">▤</span><span class="playlist-copy"><strong>{playlist.name}</strong><small>{playlist.trackCount} {playlist.trackCount === 1 ? "track" : "tracks"}</small></span>
            </button>
            <label class="move-control"><span class="visually-hidden">Move {playlist.name} to section</span>
              <select aria-label={`Move ${playlist.name}`} value={section && assignments[playlist.id] === section.id ? section.id : ""} onchange={(event) => moveFromSelector(playlist.id, event.currentTarget.value)}>
                <option value="">Unsorted</option>{#each sections as target (target.id)}<option value={target.id}>{target.name}</option>{/each}
              </select>
            </label>
          </div>
        {:else}
          {#if filterActive}
            <p class="empty-section">No matching playlists in this section.</p>
          {:else}
            <p class="empty-section">{section ? "Drop a playlist here or create one." : "No unassigned playlists."}</p>
          {/if}
        {/each}
      </div>
    </details>
  {/each}
</section>

<style>
  .playlist-sections{display:grid;gap:6px;min-width:0}.section-toolbar,.section-actions{display:flex;align-items:center}.section-toolbar{justify-content:flex-end;padding:5px 8px;color:var(--muted);font-size:10px;letter-spacing:1px;text-transform:uppercase}.playlist-sections button{border:0;border-radius:5px;background:transparent;color:var(--muted);padding:5px 7px;font:inherit;cursor:pointer}.playlist-sections button:hover{background:var(--raised);color:var(--text)}.add-section{font-size:10px!important;letter-spacing:0;text-transform:none}.playlist-section{min-width:0;border-radius:7px}.playlist-section.drag-over{outline:1px dashed var(--accent);background:color-mix(in srgb,var(--accent) 8%,transparent)}.playlist-section>summary{display:flex;align-items:center;gap:8px;min-height:34px;padding:4px 8px;border-radius:7px;color:var(--muted);cursor:pointer;list-style:none}.playlist-section>summary::-webkit-details-marker{display:none}.playlist-section>summary:hover{background:var(--raised);color:var(--text)}.playlist-section>summary small{font-size:9px;font-variant-numeric:tabular-nums}.section-name{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:11px;font-weight:650}.chevron{transition:transform 140ms ease}.playlist-section:not([open]) .chevron{transform:rotate(-90deg)}.section-content{display:grid;gap:2px;padding:2px 0 6px 8px}.section-actions{gap:2px}.section-actions button{font-size:10px}.playlist-row{display:flex;align-items:center;gap:3px;min-width:0;border-radius:6px}.playlist-row.active{background:var(--raised)}.playlist-row:focus-within,.playlist-row:hover{background:var(--raised)}.playlist-title{display:flex;align-items:center;gap:8px;flex:1;min-width:0;text-align:left!important}.playlist-title>span:first-child{flex:none;font-size:13px}.playlist-copy{display:grid;min-width:0}.playlist-copy strong,.playlist-copy small{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.playlist-copy strong{color:var(--text);font-size:11px;font-weight:560}.playlist-copy small{color:var(--muted);font-size:9px}.move-control select{max-width:90px;min-width:28px;width:28px;padding:3px;border:0;border-radius:5px;background:transparent;color:transparent;font-size:10px}.move-control select:focus{width:auto;max-width:140px;color:var(--text);background:var(--raised);outline:1px solid var(--accent)}.move-control option{color:var(--text);background:var(--surface)}.empty-section{margin:0;padding:4px 8px;color:var(--muted);font-size:10px}.section-form{display:grid;gap:7px;padding:8px;border:1px solid var(--line);border-radius:7px;background:var(--surface)}.section-form label{display:grid;gap:5px;color:var(--muted);font-size:10px}.section-form input{box-sizing:border-box;width:100%;min-width:0;padding:7px;border:1px solid var(--line);border-radius:5px;background:var(--raised);color:var(--text)}.section-form>div{display:flex;gap:4px}.section-form button{font-size:10px}.visually-hidden{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}@media(max-width:600px){.section-toolbar{padding-inline:5px}.section-content{padding-left:4px}.move-control select{max-width:65px}.section-actions{flex-wrap:wrap}}
</style>
