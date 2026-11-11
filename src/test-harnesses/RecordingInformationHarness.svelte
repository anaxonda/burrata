<script lang="ts">
  import { onMount } from "svelte";
  import RecordingInformation from "../RecordingInformation.svelte";
  import type { Track } from "../lib/types";
  let track = $state<Track>({id:"radio-song",provider:"radio_song",providerId:"one",sourceUrl:"",title:"A",artist:"Artist",favorite:false,provenance:{}});
  let contextLookupRevision = $state(0);
  onMount(() => {
    const change = (event: Event) => { track = (event as CustomEvent<Track>).detail; };
    const configChange = () => { contextLookupRevision++; };
    window.addEventListener("recording-information-test-track", change);
    window.addEventListener("recording-information-test-config", configChange);
    return () => { window.removeEventListener("recording-information-test-track", change); window.removeEventListener("recording-information-test-config", configChange); };
  });
</script>
<RecordingInformation {track} {contextLookupRevision} contextLookupConfigured={true} loadRecording={async()=>({resolution:"unresolved",recording:null,candidates:[]})} />
