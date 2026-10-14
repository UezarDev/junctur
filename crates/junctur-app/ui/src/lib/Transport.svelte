<script lang="ts">
  // The play bar (2g): its own opaque row under the picture, between the video and the track
  // lanes. Play, the skip toggle, the clocks, the whole recording as an overview (outlining the
  // window the lanes show) and, at the right end, the lanes' zoom. The track controls that were
  // here (2a's track select, 2e's strips) live on the lanes and the portrait strips; editing
  // sits on the picture (ActionBar, 2r). Review (2o) goes through the clips one at a time; while it
  // runs, Skip cuts stays off (review plays the cuts it judges).
  import * as api from "./api";
  import Timeline from "./Timeline.svelte";
  import Clocks from "./Clocks.svelte";
  import type { Selection } from "./selection";

  let {
    player,
    edit,
    spans,
    view,
    onZoom,
    reviewing,
    onReview,
    onSelect,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    /** The selection in source seconds, a span per stretch, drawn on the timeline. */
    spans: { start: number; end: number }[];
    /** The window the lanes show (source seconds). */
    view: { start: number; end: number } | null;
    /** Zoom the lanes: a factor on the window's length, or `whole` for the whole recording. */
    onZoom: (by: number | "whole") => void;
    /** Review mode is on (2o), and the toggle for it. */
    reviewing: boolean;
    onReview: () => void;
    onSelect: (s: Selection | null) => void;
    onError: (message: string) => void;
  } = $props();

  const loaded = $derived(!!player?.path);
  // Skipping needs an edit list that keeps something; otherwise the toggle says why not.
  const canSkip = $derived(loaded && !!edit?.fps && edit.ranges.length > 0);
  const canReview = $derived(loaded && !!edit?.editable && !!edit.fps);
  const skipTitle = $derived(
    canSkip
      ? "Play only what the edit list keeps (S)"
      : (edit?.error ?? "No edit list yet: it comes with the analysis"),
  );

  async function call(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }
</script>

<div class="bar">
  <button
    class="play"
    onclick={() => call(api.togglePause)}
    disabled={!loaded}
    title="Play/pause (Space)"
    aria-label={player?.paused || !loaded ? "Play" : "Pause"}
  >
    {#if player?.paused || !loaded}
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 2.5v11l9-5.5z" /></svg>
    {:else}
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 2.5h3v11H4zM9 2.5h3v11H9z" /></svg>
    {/if}
  </button>
  <button
    class="skip"
    class:on={canSkip && edit?.skip}
    onclick={() => call(() => api.setSkip(!edit?.skip))}
    disabled={!canSkip || reviewing}
    title={skipTitle}
    aria-pressed={canSkip && !!edit?.skip}
  >
    Skip cuts
  </button>
  <button
    class="skip"
    class:on={reviewing}
    onclick={onReview}
    disabled={!canReview && !reviewing}
    title={canReview || reviewing ? "Go through the clips one at a time: Keep or Remove each (R)" : (edit?.error ?? "No edit list yet: it comes with the analysis")}
    aria-pressed={reviewing}
  >
    Review
  </button>
  <Clocks {player} {edit} />
  <Timeline {player} {edit} {spans} {view} {onSelect} {onError} />
  <span class="zoom" role="group" aria-label="Zoom the track lanes">
    <button disabled={!loaded} onclick={() => onZoom(2)} title="Zoom the lanes out (Ctrl+wheel on them)" aria-label="Zoom out">−</button>
    <button disabled={!loaded} onclick={() => onZoom(0.5)} title="Zoom the lanes in (Ctrl+wheel on them)" aria-label="Zoom in">+</button>
    <button disabled={!loaded} onclick={() => onZoom("whole")} title="Show the whole recording in the lanes">All</button>
  </span>
</div>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 100%;
    padding: 0 12px;
    box-sizing: border-box;
    background: var(--panel-solid);
    border-top: 1px solid var(--line);
  }
  .play {
    display: grid;
    place-items: center;
    width: 34px;
    height: 30px;
    padding: 0;
  }
  .play svg {
    width: 14px;
    height: 14px;
    fill: currentColor;
  }
  .skip.on {
    border-color: var(--accent);
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .zoom {
    display: inline-flex;
    gap: 2px;
  }
  .zoom button {
    min-width: 28px;
    padding: 3px 6px;
  }
</style>
