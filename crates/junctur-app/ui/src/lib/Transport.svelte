<script lang="ts">
  import * as api from "./api";
  import Timeline from "./Timeline.svelte";
  import Clocks from "./Clocks.svelte";
  import EditBar from "./EditBar.svelte";
  import Mixer from "./Mixer.svelte";
  import type { Selection } from "./selection";

  let {
    player,
    edit,
    mixer,
    selection,
    span,
    right,
    onSelect,
    onOpen,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    mixer: api.MixerState | null;
    selection: Selection | null;
    /** The selection in source seconds, drawn on the timeline. */
    span: { start: number; end: number } | null;
    /** Pixels on the right taken by the transcript panel. */
    right: number;
    onSelect: (s: Selection | null) => void;
    onOpen: () => void;
    onError: (message: string) => void;
  } = $props();

  const loaded = $derived(!!player?.path);
  // Skipping needs an edit list that keeps something; otherwise the toggle says why not.
  const canSkip = $derived(loaded && !!edit?.fps && edit.ranges.length > 0);
  const skipTitle = $derived(
    canSkip
      ? "Play only what the edit list keeps (S)"
      : (edit?.error ?? "No edit list yet: run the pipeline on this recording"),
  );

  async function call(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }
</script>

<footer style:right="{right}px">
  {#if loaded}
    <EditBar {player} {edit} {selection} {onError} />
    <!-- Every track at once, with a meter and a mute each (2e); its own row, so the timeline
         keeps the play row's width however many tracks there are. -->
    <Mixer {mixer} tracks={player?.audioTracks ?? []} {onError} />
  {/if}
  <div class="row">
  <button onclick={onOpen} disabled={!player} title="Open a recording">Open</button>
  <button
    class="play"
    onclick={() => call(api.togglePause)}
    disabled={!loaded}
    title="Play/pause (Space)"
  >
    {player?.paused || !loaded ? "Play" : "Pause"}
  </button>
  <button
    class="skip"
    class:on={canSkip && edit?.skip}
    onclick={() => call(() => api.setSkip(!edit?.skip))}
    disabled={!canSkip}
    title={skipTitle}
    aria-pressed={canSkip && !!edit?.skip}
  >
    Skip cuts
  </button>
  <Clocks {player} {edit} />
  <Timeline {player} {edit} {span} {onSelect} {onError} />
  </div>
</footer>

<style>
  footer {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 16px;
    background: linear-gradient(to top, var(--panel) 60%, transparent);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .play {
    min-width: 64px;
  }
  .skip.on {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
