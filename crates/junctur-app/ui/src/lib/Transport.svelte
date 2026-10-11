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

  // While the bar is dragged it shows the drag position, not mpv's (which lags behind seeks).
  let scrub = $state<number | null>(null);
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
    oninput={(e) => {
      const to = e.currentTarget.valueAsNumber;
      scrub = to;
      call(() => api.seek(to, false));
    }}
    onchange={(e) => {
      const to = e.currentTarget.valueAsNumber;
      call(() => api.seek(to, true)).then(() => (scrub = null));
    }}
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
