<script lang="ts">
  import * as api from "./api";
  import Timeline from "./Timeline.svelte";
  import Clocks from "./Clocks.svelte";

  let {
    player,
    edit,
    onOpen,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    onOpen: () => void;
    onError: (message: string) => void;
  } = $props();

  const loaded = $derived(!!player?.path);
  const selectedTrack = $derived(player?.audioTracks.find((t) => t.selected)?.id ?? "");
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

  function trackLabel(t: api.AudioTrack): string {
    const parts = [t.title, t.lang, t.channels ? `${t.channels}ch` : null].filter(Boolean);
    return `${t.id}${parts.length ? ": " + parts.join(", ") : ""}`;
  }
</script>

<footer>
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
  <Timeline {player} {edit} {onError} />
  <select
    value={selectedTrack}
    disabled={!player?.audioTracks.length}
    title="Audio track"
    onchange={(e) => call(() => api.setAudioTrack(Number(e.currentTarget.value)))}
  >
    {#if !player?.audioTracks.length}
      <option value="">No audio</option>
    {/if}
    {#each player?.audioTracks ?? [] as t (t.id)}
      <option value={t.id}>Track {trackLabel(t)}</option>
    {/each}
  </select>
</footer>

<style>
  footer {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 16px;
    background: linear-gradient(to top, var(--panel) 60%, transparent);
  }
  .play {
    min-width: 64px;
  }
  .skip.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  select {
    max-width: 220px;
  }
</style>
