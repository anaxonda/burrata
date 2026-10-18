<script lang="ts">
  import { onDestroy, tick } from "svelte";
  import type { DirectoryBrowserRoot, DirectoryChild } from "./lib/directory-browser";

  type FolderLocation = { rootId: string; relativePath: string };
  type FolderNode = FolderLocation & { name: string; depth: number; root: boolean };
  let {
    call,
    selectedFolder = null,
    rootsRevision = 0,
    onSelectFolder = () => {},
    onRootsChanged = () => {},
    onOpenFolders = () => {},
  }: {
    call?: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    selectedFolder?: FolderLocation | null;
    rootsRevision?: number;
    onSelectFolder?: (location: FolderLocation | null) => void;
    onRootsChanged?: () => void;
    onOpenFolders?: () => void;
  } = $props();

  let roots = $state<DirectoryBrowserRoot[]>([]);
  let childrenByKey = $state<Record<string, DirectoryChild[]>>({});
  let expanded = $state<Set<string>>(new Set());
  let loading = $state<Set<string>>(new Set());
  let errors = $state<Record<string, string>>({});
  let focusedKey = $state("");
  let rootBusy = $state(false);
  let treeOpen = $state(true);
  let treeElement: HTMLElement | undefined = $state();
  let rootRequest = 0;
  let destroyed = false;
  const childRequests = new Map<string, number>();

  function keyOf(rootId: string, relativePath: string) { return `${rootId}\u0000${relativePath}`; }
  function locationOf(node: FolderNode): FolderLocation { return { rootId: node.rootId, relativePath: node.relativePath }; }
  function sameLocation(left: FolderLocation | null, right: FolderLocation) {
    return Boolean(left && left.rootId === right.rootId && left.relativePath === right.relativePath);
  }
  const visibleNodes = $derived.by(() => {
    const result: FolderNode[] = [];
    const append = (rootId: string, path: string, name: string, depth: number, root: boolean) => {
      const node = { rootId, relativePath: path, name, depth, root };
      result.push(node);
      const key = keyOf(rootId, path);
      if (expanded.has(key)) {
        for (const child of childrenByKey[key] ?? []) append(rootId, child.relativePath, child.name, depth + 1, false);
      }
    };
    for (const root of roots) append(root.id, "", root.name, 0, true);
    return result;
  });

  async function refreshRoots() {
    if (!call) { roots = []; return; }
    const request = ++rootRequest;
    for (const [key, active] of childRequests) childRequests.set(key, active + 1);
    loading = new Set();
    try {
      const next = await call<DirectoryBrowserRoot[]>("list_directory_browser_roots");
      if (destroyed || request !== rootRequest || !Array.isArray(next)) return;
      roots = next;
      const known = new Set(next.map(root => root.id));
      expanded = new Set([...expanded].filter(key => known.has(key.split("\u0000", 1)[0]!)));
      childrenByKey = Object.fromEntries(Object.entries(childrenByKey).filter(([key]) => known.has(key.split("\u0000", 1)[0]!)));
      errors = Object.fromEntries(Object.entries(errors).filter(([key]) => known.has(key.split("\u0000", 1)[0]!)));
      focusedKey = visibleNodes[0] ? keyOf(visibleNodes[0].rootId, visibleNodes[0].relativePath) : "";
    } catch {
      if (!destroyed && request === rootRequest) errors = { ...errors, roots: "Folders could not be loaded." };
    }
  }

  async function loadChildren(rootId: string, relativePath: string) {
    if (!call) return;
    const key = keyOf(rootId, relativePath);
    if (loading.has(key)) return;
    const request = (childRequests.get(key) ?? 0) + 1;
    childRequests.set(key, request);
    loading = new Set(loading).add(key);
    errors = { ...errors, [key]: "" };
    try {
      const response = await call<{ rootId: string; relativePath: string; directories: DirectoryChild[] }>("list_directory_browser_children", { rootId, relativePath });
      if (destroyed || childRequests.get(key) !== request || response.rootId !== rootId || response.relativePath !== relativePath || !roots.some(root => root.id === rootId)) return;
      childrenByKey = { ...childrenByKey, [key]: response.directories };
    } catch (cause) {
      if (!destroyed && childRequests.get(key) === request) errors = { ...errors, [key]: cause instanceof Error ? cause.message : String(cause) };
    } finally {
      if (!destroyed && childRequests.get(key) === request) {
        const next = new Set(loading); next.delete(key); loading = next;
      }
    }
  }

  function toggle(node: FolderNode) {
    const key = keyOf(node.rootId, node.relativePath);
    if (expanded.has(key)) expanded = new Set([...expanded].filter(item => item !== key));
    else {
      expanded = new Set(expanded).add(key);
      if (!(key in childrenByKey)) void loadChildren(node.rootId, node.relativePath);
    }
  }

  async function focusNode(node: FolderNode) {
    focusedKey = keyOf(node.rootId, node.relativePath);
    await tick();
    const target = [...(treeElement?.querySelectorAll<HTMLElement>("[role=treeitem]") ?? [])]
      .find(item => item.dataset.nodeKey === focusedKey);
    target?.focus();
  }

  async function handleKeydown(event: KeyboardEvent, node: FolderNode, index: number) {
    const key = keyOf(node.rootId, node.relativePath);
    const isExpanded = expanded.has(key);
    let destination: FolderNode | undefined;
    if (event.key === "ArrowDown") destination = visibleNodes[Math.min(index + 1, visibleNodes.length - 1)];
    else if (event.key === "ArrowUp") destination = visibleNodes[Math.max(index - 1, 0)];
    else if (event.key === "Home") destination = visibleNodes[0];
    else if (event.key === "End") destination = visibleNodes[visibleNodes.length - 1];
    else if (event.key === "ArrowRight") {
      if (!isExpanded) { toggle(node); event.preventDefault(); return; }
      destination = visibleNodes[index + 1]?.depth === node.depth + 1 ? visibleNodes[index + 1] : undefined;
    } else if (event.key === "ArrowLeft") {
      if (isExpanded) { toggle(node); event.preventDefault(); return; }
      destination = visibleNodes.slice(0, index).reverse().find(item => item.rootId === node.rootId && item.depth === node.depth - 1);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault(); onSelectFolder(locationOf(node)); return;
    } else return;
    event.preventDefault();
    if (destination) await focusNode(destination);
  }

  $effect(() => { rootsRevision; void refreshRoots(); });
  onDestroy(() => {
    destroyed = true;
    rootRequest++;
    for (const [key, request] of childRequests) childRequests.set(key, request + 1);
  });

  async function addFolder() {
    if (!call || rootBusy) return;
    rootBusy = true;
    try {
      const choice = await call<{ path: string } | null>("choose_directory_browser_root");
      if (!choice?.path) return;
      const root = await call<DirectoryBrowserRoot>("add_directory_browser_root", { path: choice.path });
      await refreshRoots();
      onRootsChanged();
      onSelectFolder({ rootId: root.id, relativePath: "" });
      const rootKey = keyOf(root.id, "");
      focusedKey = rootKey;
    } catch (cause) {
      errors = { ...errors, roots: cause instanceof Error ? cause.message : String(cause) };
    } finally {
      rootBusy = false;
    }
  }

  async function removeFolder(node: FolderNode) {
    if (!call || rootBusy || !node.root) return;
    rootBusy = true;
    const rootId = node.rootId;
    try {
      await call("remove_directory_browser_root", { rootId });
      const wasSelected = selectedFolder?.rootId === rootId;
      await refreshRoots();
      onRootsChanged();
      if (wasSelected) {
        const nextRoot = roots[0];
        if (nextRoot) onSelectFolder({ rootId: nextRoot.id, relativePath: "" });
        else onSelectFolder(null);
      }
    } catch (cause) {
      errors = { ...errors, roots: cause instanceof Error ? cause.message : String(cause) };
    } finally {
      rootBusy = false;
    }
  }
</script>

<section class="folder-tree" aria-labelledby="folder-tree-title">
  <div class="folder-tree-heading">
    <button class="folder-tree-title" id="folder-tree-title" type="button" aria-label="Folders" onclick={onOpenFolders}>Folders</button>
    <div class="folder-tree-heading-actions">
      <button type="button" class="folder-tree-collapse" aria-label={treeOpen ? "Collapse folder tree" : "Expand folder tree"} aria-expanded={treeOpen} title={treeOpen ? "Collapse folder tree" : "Expand folder tree"} onclick={() => treeOpen = !treeOpen}>{treeOpen ? "⌃" : "⌄"}</button>
      <button type="button" aria-label="Add folder" title="Add folder" disabled={rootBusy} onclick={() => void addFolder()}>＋</button>
    </div>
  </div>
  {#if errors.roots}<p class="folder-tree-error" role="status">{errors.roots}</p>{/if}
  {#if treeOpen && roots.length}
    <div class="folder-tree-list" role="tree" aria-label="Registered folders" bind:this={treeElement}>
      {#each visibleNodes as node, index (keyOf(node.rootId, node.relativePath))}
        {@const key = keyOf(node.rootId, node.relativePath)}
        {@const isExpanded = expanded.has(key)}
        {@const isLoading = loading.has(key)}
        <div class="folder-tree-node" role="none" style={`--folder-depth:${node.depth}`}>
          <button type="button" role="treeitem" tabindex={focusedKey === key || (!focusedKey && index === 0) ? 0 : -1}
            data-node-key={key} aria-level={node.depth + 1} aria-expanded={isExpanded} aria-busy={isLoading}
            aria-selected={sameLocation(selectedFolder, locationOf(node))}
            aria-current={sameLocation(selectedFolder, locationOf(node)) ? "location" : undefined}
            class:selected={sameLocation(selectedFolder, locationOf(node))}
            aria-label={`${node.name}${isLoading ? ", loading subfolders" : ""}`}
            onclick={(event) => {
              if ((event.target as HTMLElement).closest(".folder-tree-chevron")) { event.preventDefault(); toggle(node); return; }
              onSelectFolder(locationOf(node));
            }}
            onkeydown={(event) => void handleKeydown(event, node, index)}>
            <span class="folder-tree-chevron" aria-hidden="true">{isLoading ? "…" : isExpanded ? "⌄" : "›"}</span>
            <span class="folder-tree-icon" aria-hidden="true">{node.root ? "▣" : "▧"}</span>
            <span class="folder-tree-name" title={node.name}>{node.name}</span>
          </button>
          {#if node.root}<button class="folder-tree-remove" type="button" disabled={rootBusy} aria-label={`Unregister folder ${node.name}`} title={`Unregister ${node.name}; files are not deleted`} onclick={() => void removeFolder(node)}>×</button>{/if}
          {#if errors[key]}<span class="folder-tree-error" role="status">{errors[key]}</span>{/if}
        </div>
      {/each}
    </div>
  {:else if treeOpen && !errors.roots}
    <p class="folder-tree-empty">No folders registered</p>
  {/if}
</section>

<style>
  .folder-tree { margin: 4px 0 12px; min-width: 0; }
  .folder-tree-heading { display: flex; align-items: center; justify-content: space-between; gap: 6px; padding: 2px 2px 5px 8px; color: var(--muted, #9a9da9); font-size: .72rem; letter-spacing: .08em; text-transform: uppercase; }
  .folder-tree-heading-actions { display: flex; align-items: center; gap: 1px; }
  .folder-tree-heading button { border: 0; background: transparent; color: inherit; border-radius: 5px; min-width: 24px; height: 24px; cursor: pointer; font-size: 1rem; }
  .folder-tree-heading button:hover, .folder-tree-heading button:focus-visible { background: var(--surface-hover, #ffffff12); color: var(--text, #fff); }
  .folder-tree-heading .folder-tree-title { min-width: 0; padding: 2px 4px; text-align: left; font: inherit; letter-spacing: inherit; text-transform: inherit; }
  .folder-tree-list { display: grid; gap: 1px; max-height: 220px; overflow: auto; }
  .folder-tree-node { min-width: 0; position: relative; display: flex; align-items: center; }
  .folder-tree-node [role=treeitem] { flex: 1; width: 100%; min-width: 0; min-height: 29px; display: flex; align-items: center; gap: 5px; padding: 3px 28px 3px calc(7px + var(--folder-depth) * 13px); border: 0; border-radius: 6px; background: transparent; color: var(--text, #e5e7eb); text-align: left; cursor: pointer; font: inherit; font-size: .82rem; }
  .folder-tree-node [role=treeitem]:hover, .folder-tree-node [role=treeitem]:focus-visible { background: var(--surface-hover, #ffffff12); outline: none; }
  .folder-tree-node [role=treeitem].selected { background: var(--accent-soft, #ffffff16); color: var(--accent, #f1f1f1); }
  .folder-tree-chevron { width: 12px; flex: 0 0 12px; text-align: center; color: var(--muted, #9a9da9); cursor: pointer; }
  .folder-tree-icon { flex: 0 0 13px; color: var(--muted, #9a9da9); }
  .folder-tree-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .folder-tree-remove { position: absolute; right: 3px; width: 23px; height: 23px; border: 0; border-radius: 5px; background: transparent; color: var(--muted, #9a9da9); cursor: pointer; }
  .folder-tree-remove:hover, .folder-tree-remove:focus-visible { background: var(--surface-hover, #ffffff12); color: var(--text, #fff); }
  .folder-tree-empty, .folder-tree-error { margin: 3px 8px; color: var(--muted, #9a9da9); font-size: .76rem; }
  .folder-tree-error { color: var(--danger, #e89a9a); }
</style>
