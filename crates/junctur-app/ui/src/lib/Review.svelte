<script lang="ts">
  // Review mode (2o): the proposed edit gone through one clip at a time, as Uezar asked ("like
  // Tinder", buttons rather than swipes). The clips are review.ts's, frozen as it starts; each
  // plays from its start on mpv's own loop until it is decided. Keep and Remove are one whole-clip
  // frame edit each, the same undoable edit the range toolbar's Cut range / Keep cut makes, then
  // the next clip plays; Back and Skip move without deciding. It sits in the range toolbar's one
  // place over the lanes (2n), which hides while it shows. A cut clip is what is being judged, so
  // review plays the whole file: it turns Skip cuts off and puts it back as it was when it ends,
  // and the speed back to 1x (there is no other speed control). App hands it the keys first.
  import { onMount, untrack } from "svelte";
  import * as api from "./api";
  import { clips, holding, now, SPEEDS, type Clip } from "./review";
  import { timecode, toFrame, toSeconds } from "./time";

  let {
    player,
    edit,
    at,
    onDone,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    /** Where to sit, as ActionBar's `at`. */
    at: { x: number; min: number; max: number; bottom: number };
    onDone: () => void;
    onError: (message: string) => void;
  } = $props();

  // Frozen as review starts (see review.ts); `index` is the clip shown, `list.length` once past
  // the last one.
  const start = untrack(() => edit);
  const fps = start?.fps ?? null;
  const list: Clip[] = start && fps ? clips(start) : [];
  const total = start?.totalFrames ?? 0;
  const from = untrack(() => player?.frame ?? (fps && player?.position != null ? toFrame(player.position, fps) : 0));
  let index = $state(holding(list, from));
  let speed = $state(1);
  // An edit or a move on its way: a second click waits for it, so two decisions never cross.
  let busy = $state(false);
  let width = $state(0);

  const clip = $derived(index < list.length ? list[index] : null);
  const live = $derived(clip && edit ? now(edit, clip) : null);
  const changed = $derived(clip && live ? (clip.kept ? live !== "kept" : live !== "cut") : false);
  const left = $derived(Math.round(Math.min(Math.max(at.x - width / 2, at.min), Math.max(at.min, at.max - width))));

  function length(c: Clip): string {
    if (!fps) return "";
    const s = toSeconds(c.end - c.start, fps);
    if (s < 60) return `${s.toFixed(1)} s`;
    const m = Math.floor(s / 60);
    return `${m}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
  }

  async function call(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }

  function play(c: Clip): Promise<void> {
    return fps ? api.playBetween(toSeconds(c.start, fps), toSeconds(c.end, fps)) : Promise.resolve();
  }

  // Shows clip `to` and plays it; past the last one the loop ends and playback pauses.
  async function go(to: number) {
    index = Math.max(0, Math.min(list.length, to));
    if (index < list.length) return play(list[index]);
    await api.endLoop();
    if (player && !player.paused) await api.togglePause();
  }

  async function step(f: () => Promise<void>) {
    if (busy) return;
    busy = true;
    try {
      await f();
    } catch (e) {
      onError(String(e));
    } finally {
      busy = false;
    }
  }

  // Keep (true) or Remove the clip shown, then the next. One already so sends no edit, so the
  // undo history holds only real changes.
  function decide(keep: boolean) {
    const c = clip;
    if (!c) return;
    void step(async () => {
      const is = edit ? now(edit, c) : null;
      if (is !== (keep ? "kept" : "cut")) await api.editFrames(c.start, c.end, keep);
      await go(index + 1);
    });
  }
  const back = () => void step(() => go(index - 1));
  const skip = () => void step(() => go(index + 1));
  function setSpeed(x: number) {
    speed = x;
    void call(() => api.setSpeed(x));
  }

  /** A key while reviewing: true when review took it. Ctrl combinations (undo, redo, the menus)
   * stay the editor's. */
  export function key(e: KeyboardEvent): boolean {
    if (e.ctrlKey || e.metaKey || e.altKey) return false;
    switch (e.code) {
      case "KeyK":
      case "ArrowRight":
        decide(true);
        return true;
      case "Delete":
      case "Backspace":
      case "ArrowLeft":
        decide(false);
        return true;
      case "ArrowUp":
        back();
        return true;
      case "ArrowDown":
        skip();
        return true;
      case "Space":
        if (clip) void call(api.togglePause);
        return true;
      case "Escape":
      case "KeyR":
        onDone();
        return true;
      case "KeyS":
        // Skip cuts stays off while reviewing (the loop is in source time).
        return true;
    }
    const n = /^Digit([1-9])$/.exec(e.code);
    if (n && Number(n[1]) <= SPEEDS.length) {
      setSpeed(SPEEDS[Number(n[1]) - 1]);
      return true;
    }
    return false;
  }

  // Another list under review (a re-analysis wrote a new one): its frames are not these clips'.
  $effect(() => {
    if (edit && edit.totalFrames !== total) untrack(onDone);
  });

  onMount(() => {
    const skipped = !!start?.skip;
    let gone = false;
    void (async () => {
      try {
        if (skipped) {
          await api.setSkip(false);
          // The whole file loads again: a seek sent before it is playing would be lost.
          for (let i = 0; i < 60 && !gone; i++) {
            const s = await api.playerState();
            if (!s.cuts && s.position != null) break;
            await new Promise((r) => setTimeout(r, 50));
          }
        }
        if (!gone && index < list.length) await play(list[index]);
      } catch (e) {
        onError(String(e));
      }
    })();
    return () => {
      gone = true;
      void api.endLoop().catch(() => {});
      void api.setSpeed(1).catch(() => {});
      if (skipped) void api.setSkip(true).catch(() => {});
    };
  });
</script>

<div class="float" style:left="{left}px" style:bottom="{at.bottom}px" bind:clientWidth={width}>
  <div class="bar" role="group" aria-label="Review clips">
    <span class="where mono">{Math.min(index + 1, list.length)} of {list.length}</span>
    {#if clip && fps}
      <span class="kind" class:cut={!clip.kept}>{clip.kept ? `Range ${clip.index}` : "Cut"}</span>
      {#if changed}<span class="now">now {live === "partly" ? "partly kept" : live}</span>{/if}
      <span class="mono">{timecode(clip.start, fps)} – {timecode(clip.end, fps)}</span>
      <span class="len mono">{length(clip)}</span>
    {:else}
      <span class="all">{list.length ? `All ${list.length} clips reviewed` : "No clips to review"}</span>
    {/if}
    <button disabled={busy || index === 0} onclick={back} title="The clip before (up arrow)">Back</button>
    <button class="remove" disabled={busy || !clip} onclick={() => decide(false)} title="Cut this clip and go on (left arrow, Delete)"
      >Remove</button
    >
    <button class="keep" disabled={busy || !clip} onclick={() => decide(true)} title="Keep this clip and go on (right arrow, K)">Keep</button>
    <button disabled={busy || !clip} onclick={skip} title="The next clip, deciding nothing (down arrow)">Skip</button>
    <span class="speeds" role="group" aria-label="Playback speed">
      {#each SPEEDS as x, i (x)}
        <button class:on={speed === x} aria-pressed={speed === x} onclick={() => setSpeed(x)} title="Play at {x}x ({i + 1})">{x}x</button>
      {/each}
    </span>
    <button class="i" disabled={!edit?.canUndo} onclick={() => call(api.undo)} title="Undo (Ctrl+Z)" aria-label="Undo">↶</button>
    <button class="i" disabled={!edit?.canRedo} onclick={() => call(api.redo)} title="Redo (Ctrl+Y)" aria-label="Redo">↷</button>
    <button onclick={onDone} title="Leave review (Esc, R)">Done</button>
  </div>
</div>

<style>
  .float {
    position: absolute;
    z-index: 5;
    max-width: 90vw;
  }
  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: 5px 7px;
    border-radius: 8px;
    background: rgba(28, 29, 34, 0.96);
    border: 1px solid var(--accent);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.45);
    white-space: nowrap;
  }
  .where {
    padding: 0 4px;
    color: var(--muted);
  }
  .kind {
    color: var(--good);
  }
  .kind.cut {
    color: var(--bad);
  }
  .now,
  .len,
  .all {
    color: var(--muted);
    font-size: 12px;
  }
  .mono {
    font-size: 12px;
  }
  button {
    padding: 3px 9px;
  }
  .keep,
  .remove {
    padding: 4px 16px;
    font-weight: 600;
  }
  .keep:not(:disabled) {
    border-color: var(--good);
    color: var(--good);
  }
  .remove:not(:disabled) {
    border-color: var(--bad);
    color: var(--bad);
  }
  .speeds {
    display: inline-flex;
    gap: 2px;
  }
  .speeds button {
    padding: 3px 6px;
    font-size: 12px;
  }
  .speeds .on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .i {
    padding: 2px 8px;
  }
</style>
