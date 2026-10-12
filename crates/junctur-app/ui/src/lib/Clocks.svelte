<script lang="ts">
  // Source time and edited (output) time side by side. With an edit list both are timecode at the
  // source frame rate, rounded the way the render and FCPXML round, so the frame shown is the frame
  // named; without one, the plain clock of 2a.
  import type { EditState, PlayerState } from "./api";
  import { clock, timecode } from "./time";

  let { player, edit }: { player: PlayerState | null; edit: EditState | null } = $props();

  const fps = $derived(edit?.fps ?? null);
  const loaded = $derived(!!player?.path);
</script>

<div class="clocks mono">
  {#if fps && player?.output != null}
    <span title="Source time">
      <b>SRC</b>
      {timecode(loaded ? player?.position : null, fps)} / {timecode(player?.duration, fps)}
    </span>
    <span title="Edited (output) time: the same frame in the render and in Resolve">
      <b>EDIT</b>
      {timecode(player.output, fps)} / {timecode(player.outputDuration, fps)}
      {#if player.inCut}<em>in cut</em>{/if}
    </span>
  {:else}
    <span>{clock(loaded ? player?.position : null)} / {clock(player?.duration)}</span>
  {/if}
</div>

<style>
  .clocks {
    display: flex;
    flex-direction: column;
    gap: 1px;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }
  b {
    display: inline-block;
    width: 34px;
    font-weight: 600;
    color: var(--text);
    opacity: 0.6;
  }
  em {
    margin-left: 6px;
    font-style: normal;
    color: var(--bad);
  }
</style>
