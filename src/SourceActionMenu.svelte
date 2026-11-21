<script lang="ts">
  import ActionMenu from "./ActionMenu.svelte";
  import MenuIcon from "./MenuIcon.svelte";
  import type { MenuAnchorRect } from "./lib/menu-geometry";

  let { title, url, sourceLabel }: { title: string; url: string; sourceLabel: string } = $props();
  let anchor = $state<MenuAnchorRect | null>(null);
</script>

<button class="source-actions-trigger" type="button" aria-label={`More actions for ${title}`}
  title="More actions" aria-haspopup="dialog" onclick={(event) => {
    event.currentTarget.focus();
    anchor = event.currentTarget.getBoundingClientRect();
  }}>⋯</button>
{#if anchor}
  <ActionMenu label={`Actions for ${title}`} anchorRect={anchor} onClose={() => anchor = null}>
    <div class="source-row">
      <a class="source-link" href={url} target="_blank" rel="noreferrer" onclick={() => anchor = null}>
        <MenuIcon name="external" /><span>{sourceLabel}</span>
      </a>
    </div>
  </ActionMenu>
{/if}

<style>
  button { display:grid; place-items:center; box-sizing:border-box; min-width:34px; min-height:34px; padding:0 .4rem; font-size:1.2rem; line-height:1; }
</style>
