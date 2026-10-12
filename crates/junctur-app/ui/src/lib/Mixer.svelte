<script lang="ts">
  // Every audio track plays at once (2e): one strip per track with its peak meter; a click mutes or
  // unmutes it. The meters show each track's own level, muted or not, so who is talking stays
  // visible. They are written straight to the bars' styles, outside Svelte's reactivity: levels
  // come 20 times a second and would otherwise update the component each time.
  import { onMount } from "svelte";
  import * as api from "./api";

  let {
    mixer,
    tracks,
    onError,
  }: {
    mixer: api.MixerState | null;
    /** mpv's list, for the tracks' own titles. */
    tracks: api.AudioTrack[];
    onError: (message: string) => void;
  } = $props();

  /** The meter's range, in dBFS. */
  const FLOOR = -60;
  /** No levels for this long (paused, or no audio): the meters drop. */
  const QUIET_MS = 250;

  const ROLE: Record<api.Role, string> = {
    mix: "Mix",
    voice: "Voice",
    game_pc: "Game",
    music: "Music",
    silent: "Silent",
    unknown: "Track",
  };

  const bars: HTMLElement[] = [];

  function name(t: api.MixTrack): string {
    const title = tracks.find((a) => a.id === t.index + 1)?.title;
    if (title) return title;
    if (t.copyOf !== null) return `copy of ${t.copyOf + 1}`;
    return t.role ? ROLE[t.role] : "Track";
  }

  function describe(t: api.MixTrack): string {
    const what = t.copyOf !== null ? `a copy of track ${t.copyOf + 1}` : t.role ? ROLE[t.role] : "role not known yet";
    return `Track ${t.index + 1}: ${what}. ${t.muted ? "Muted: click to hear it" : "Click to mute it"} (preview only)`;
  }

  function show(levels: (number | null)[]) {
    levels.forEach((db, i) => {
      const bar = bars[i];
      if (!bar) return;
      const share = db === null ? 0 : Math.min(1, Math.max(0, (db - FLOOR) / -FLOOR));
      bar.style.transform = `scaleX(${share})`;
    });
  }

  async function toggle(t: api.MixTrack) {
    try {
      await api.setMuted(t.index, !t.muted);
    } catch (e) {
      onError(String(e));
    }
  }

  onMount(() => {
    let last = 0;
    const unlisten = api.onLevels((levels) => {
      last = performance.now();
      show(levels);
    });
    const quiet = window.setInterval(() => {
      if (last && performance.now() - last > QUIET_MS) {
        last = 0;
        show(bars.map(() => null));
      }
    }, 100);
    return () => {
      clearInterval(quiet);
      unlisten.then((f) => f());
    };
  });
</script>

{#if mixer?.tracks.length}
  <div class="mixer" role="group" aria-label="Audio tracks">
    {#each mixer.tracks as t (t.index)}
      <button
        class="strip"
        class:muted={t.muted}
        title={describe(t)}
        aria-pressed={!t.muted}
        onclick={() => toggle(t)}
      >
        <span class="label"><b>{t.index + 1}</b> {name(t)}</span>
        <span class="meter"><span class="bar" bind:this={bars[t.index]}></span></span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .mixer {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .strip {
    display: flex;
    flex-direction: column;
    gap: 3px;
    width: 84px;
    padding: 3px 6px 4px;
    text-align: left;
  }
  .label {
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .label b {
    color: var(--accent);
  }
  .meter {
    display: block;
    height: 4px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }
  .bar {
    display: block;
    height: 100%;
    background: var(--good);
    transform: scaleX(0);
    transform-origin: left;
    transition: transform 80ms linear;
  }
  .muted .label {
    color: var(--muted);
    text-decoration: line-through;
  }
  .muted .label b {
    color: var(--muted);
  }
  .muted .bar {
    background: var(--muted);
  }
</style>
