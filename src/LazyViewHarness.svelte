<script lang="ts">
  import LazyView from "./LazyView.svelte";

  let title = $state("Before update");
  let loads = $state(0);
  async function loadFixture() {
    loads += 1;
    return (await import("./LazyView.fixture.svelte")).default;
  }
</script>

<button type="button" onclick={() => title = "After update"}>Update parent</button>
<span data-testid="load-count">{loads}</span>
<LazyView loader={() => loadFixture()} componentProps={{ title }} label="fixture" />
