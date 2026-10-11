<script lang="ts">
  import * as api from "./api";
  import { clock } from "./time";

  let {
    player,
    onOpen,
    onError,
  }: {
    player: api.PlayerState | null;
    onOpen: () => void;
    onError: (message: string) => void;
  } = $props();

  // While the bar is dragged, and until mpv's position catches up with the seek, the bar shows
  // the drag position: mpv's own lags behind seeks, and showing it would make the thumb jump back.
  let scrub = $state<number | null>(null);
  // Keyframe seeks while dragging, at most one per animation frame (input fires far more often).
  let queued: number | null = null;
  let frame = 0;
  // When to give up waiting for mpv's position to reach the target (a seek that lands far off).
  let scrubUntil = 0;
  const loaded = $derived(!!player?.path);
  const duration = $derived(player?.duration ?? 0);
  const shown = $derived(scrub ?? player?.position ?? 0);
  const selectedTrack = $derived(player?.audioTracks.find((t) => t.selected)?.id ?? "");

  async function call(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }

  function dragTo(to: number) {
    scrub = to;
    queued = to;
    if (!frame) {
      frame = requestAnimationFrame(() => {
        frame = 0;
        const t = queued;
        queued = null;
        if (t != null) call(() => api.seek(t, false));
      });
    }
  }

  function release(to: number) {
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    queued = null;
    scrub = to;
    scrubUntil = performance.now() + 1500;
    call(() => api.seek(to, true));
    // If no position update comes at all, let go of the drag position anyway.
    setTimeout(() => {
      if (scrubUntil !== 0 && performance.now() >= scrubUntil) {
        scrub = null;
        scrubUntil = 0;
      }
    }, 1600);
  }

  $effect(() => {
    const pos = player?.position;
    if (scrub == null || queued != null || frame || scrubUntil === 0) return;
    if ((pos != null && Math.abs(pos - scrub) < 0.5) || performance.now() > scrubUntil) {
      scrub = null;
      scrubUntil = 0;
    }
  });

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
  <span class="mono">{clock(loaded ? shown : null)}</span>
  <input
    type="range"
    min="0"
    max={duration || 1}
    step="0.01"
    value={shown}
    disabled={!loaded || !duration}
    oninput={(e) => dragTo(e.currentTarget.valueAsNumber)}
    onchange={(e) => release(e.currentTarget.valueAsNumber)}
  />
  <span class="mono">{clock(player?.duration)}</span>
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
  input[type="range"] {
    flex: 1;
    accent-color: var(--accent);
    min-width: 80px;
  }
  span {
    color: var(--muted);
    min-width: 56px;
    text-align: center;
  }
  select {
    max-width: 220px;
  }
</style>
