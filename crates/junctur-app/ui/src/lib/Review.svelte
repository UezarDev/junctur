<script lang="ts">
  // Review mode (2o): the proposed edit gone through one clip at a time, as Uezar asked ("like
  // Tinder", buttons rather than swipes). The clips are review.ts's, frozen as it starts; each
  // plays from its start on mpv's own loop until it is decided. Keep and Remove are one whole-clip
  // frame edit each, the same undoable edit the range toolbar's Cut range / Keep cut makes, then
  // the next clip plays; Back and Skip move without deciding (Enter or K keeps, Backspace or Delete
  // removes, 2r). It is one box of one size (layout.ts `REVIEW`, a little bigger than the range
  // toolbar's) in the toolbar's corner of the picture, which hides while it shows (2r; 2o had it
  // over the lanes, sized by its contents and wrapping). A cut clip is what is being judged, so
  // review plays the whole file: the player (`play_between`) turns Skip cuts off for it and
  // `end_review` puts it back as it was and the speed back to 1x (there is no other speed control).
  // App hands it the keys first.
  import { onMount, untrack } from "svelte";
  import * as api from "./api";
  import { clips, holding, now, SPEEDS, type Clip } from "./review";
  import { clock, timecode, toFrame, toSeconds } from "./time";
  import { REVIEW } from "./layout";

  let {
    player,
    edit,
    at,
    onDone,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    /** Where the box's top-left goes (layout.ts `floatAt`). */
    at: { left: number; top: number; scale: number };
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

  const clip = $derived(index < list.length ? list[index] : null);
  const live = $derived(clip && edit ? now(edit, clip) : null);
  const changed = $derived(clip && live ? (clip.kept ? live !== "kept" : live !== "cut") : false);
  // The recording under review: another one opening (or a close) ends it.
  const path = untrack(() => player?.path ?? null);
  let gone = false;

  function length(c: Clip): string {
    if (!fps) return "";
    const s = toSeconds(c.end - c.start, fps);
    return s < 60 ? `${s.toFixed(1)} s` : clock(s);
  }

  // Every player call of the review in one line, in order (2o repair): its start, each move or
  // edit, and its end, so the end never overtakes a start or an edit still on its way. The
  // player keeps the review's own state (the skip toggle before it, the loop, the speed), and an
  // open or close ends it there too, so nothing here can land on another recording's toggle.
  let line: Promise<void> = Promise.resolve();
  function queue(f: () => Promise<void>): Promise<void> {
    const p = line.then(f);
    line = p.catch(() => {});
    return p.catch((e) => {
      if (!gone) onError(String(e));
    });
  }

  function play(c: Clip): Promise<void> {
    return fps ? api.playBetween(toSeconds(c.start, fps), toSeconds(c.end, fps)) : Promise.resolve();
  }

  // Shows clip `to` and plays it; past the last one the loop ends and playback pauses.
  function go(to: number): Promise<void> {
    index = Math.max(0, Math.min(list.length, to));
    return index < list.length ? play(list[index]) : api.pauseReview();
  }

  // A decision, a move, an undo or a redo: one at a time (`busy` greys the buttons and makes keys
  // wait their turn by dropping them), each after the one before in the line.
  function step(f: () => Promise<void>) {
    if (busy || gone) return;
    busy = true;
    void queue(f).finally(() => (busy = false));
  }

  // Keep (true) or Remove the clip shown, then the next. One already so sends no edit, so the
  // undo history holds only real changes; what it is now is asked of the engine, not taken from
  // the last edit event, which may not have come in yet.
  function decide(keep: boolean) {
    const c = clip;
    if (!c) return;
    step(async () => {
      const is = now(await api.editState(), c);
      if (is !== (keep ? "kept" : "cut")) await api.editFrames(c.start, c.end, keep);
      await go(list.indexOf(c) + 1);
    });
  }
  const back = () => step(() => go(index - 1));
  const skip = () => step(() => go(index + 1));
  const undo = () => step(api.undo);
  const redo = () => step(api.redo);
  function setSpeed(x: number) {
    speed = x;
    void queue(() => api.setSpeed(x));
  }
  // A clicked button loses the focus, so Enter or Space on the next clip is not a second click.
  function press(e: MouseEvent, f: () => void) {
    (e.currentTarget as HTMLElement).blur();
    f();
  }

  let root = $state<HTMLElement | null>(null);
  function focused(): boolean {
    const f = document.activeElement;
    return f instanceof HTMLButtonElement && !!root?.contains(f);
  }

  /** A key while reviewing: true when review took it. Undo and redo are review's too (they wait
   * for a decision on its way); other Ctrl combinations (the menus) stay the editor's. */
  export function key(e: KeyboardEvent): boolean {
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && !e.altKey && (e.code === "KeyY" || (e.code === "KeyZ" && e.shiftKey))) {
      if (edit?.canRedo) redo();
      return true;
    }
    if (ctrl && !e.altKey && e.code === "KeyZ") {
      if (edit?.canUndo) undo();
      return true;
    }
    if (ctrl || e.altKey) return false;
    switch (e.code) {
      // A held key decides once (2r): holding Enter must not keep clip after clip.
      case "Enter":
      case "NumpadEnter":
        // A review button reached with Tab takes its own Enter (Done ends review, Back goes back);
        // focus anywhere else (the play bar's Review button after the click that started it) keeps.
        if (focused()) return false;
        if (!e.repeat) decide(true);
        return true;
      case "KeyK":
      case "ArrowRight":
        if (!e.repeat) decide(true);
        return true;
      case "Backspace":
      case "Delete":
      case "ArrowLeft":
        if (!e.repeat) decide(false);
        return true;
      case "ArrowUp":
        back();
        return true;
      case "ArrowDown":
        skip();
        return true;
      case "Space":
        if (clip) void queue(api.togglePause);
        return true;
      case "Escape":
      case "KeyR":
        onDone();
        return true;
      case "KeyS":
        // Skip cuts stays off while reviewing (the player refuses it too).
        return true;
    }
    const n = /^Digit([1-9])$/.exec(e.code);
    if (n && Number(n[1]) <= SPEEDS.length) {
      setSpeed(SPEEDS[Number(n[1]) - 1]);
      return true;
    }
    return false;
  }

  // Another list under review (a re-analysis wrote a new one) or another recording open: their
  // frames are not these clips'.
  $effect(() => {
    if ((edit && edit.totalFrames !== total) || (player?.path ?? null) !== path) untrack(onDone);
  });

  onMount(() => {
    // The player turns Skip cuts off and loads the whole file at the clip's start if it has to.
    if (index < list.length) void queue(() => play(list[index]));
    return () => {
      gone = true;
      void queue(api.endReview);
    };
  });
</script>

<div bind:this={root} class="float" role="group" aria-label="Review clips" style:left="{at.left}px" style:top="{at.top}px" style:width="{REVIEW.w}px" style:height="{REVIEW.h}px" style:transform={at.scale < 1 ? `scale(${at.scale})` : undefined}>
  <div class="row info">
    {#if clip && fps}
      <span class="where mono" title="Clip {index + 1} of {list.length}">{index + 1} of {list.length}</span>
      <span class="kind" class:cut={!clip.kept}>{clip.kept ? `Range ${clip.index}` : "Cut"}</span>
      <span class="times mono">{timecode(clip.start, fps)} – {timecode(clip.end, fps)}</span>
      <span class="len mono" title={length(clip)}>{length(clip)}</span>
    {:else}
      <span class="all">{list.length ? `All ${list.length} clips reviewed` : "No clips to review"}</span>
    {/if}
  </div>
  <div class="row decide">
    <button disabled={busy || index === 0} onclick={(e) => press(e, back)} title="The clip before (up arrow)">Back</button>
    <button class="remove" disabled={busy || !clip} onclick={(e) => press(e, () => decide(false))} title="Cut this clip and go on (Backspace, Delete, left arrow)"
      >Remove</button
    >
    <button class="keep" disabled={busy || !clip} onclick={(e) => press(e, () => decide(true))} title="Keep this clip and go on (Enter, K, right arrow)">Keep</button>
    <button disabled={busy || !clip} onclick={(e) => press(e, skip)} title="The next clip, deciding nothing (down arrow)">Skip</button>
  </div>
  <div class="row more">
    <span class="now" title={changed ? `This clip is now ${live === "partly" ? "partly kept" : live}` : undefined}>{changed ? `now ${live === "partly" ? "partly kept" : live}` : ""}</span>
    <span class="speeds" role="group" aria-label="Playback speed">
      {#each SPEEDS as x, i (x)}
        <button class:on={speed === x} aria-pressed={speed === x} onclick={(e) => press(e, () => setSpeed(x))} title="Play at {x}x ({i + 1})">{x}x</button>
      {/each}
    </span>
    <button class="i" disabled={busy || !edit?.canUndo} onclick={(e) => press(e, undo)} title="Undo (Ctrl+Z)" aria-label="Undo">↶</button>
    <button class="i" disabled={busy || !edit?.canRedo} onclick={(e) => press(e, redo)} title="Redo (Ctrl+Y)" aria-label="Redo">↷</button>
    <button class="done" onclick={onDone} title="Leave review (Esc, R)">Done</button>
  </div>
</div>

<style>
  /* Drawn at exactly REVIEW's size (border and padding inside), which floatAt places by. */
  .float {
    position: absolute;
    z-index: 5;
    /* floatAt's scale, in a video area smaller than the box, from the corner it places. */
    transform-origin: top left;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 8px;
    padding: 10px 14px;
    border-radius: 18px;
    background: rgba(28, 29, 34, 0.94);
    border: 1px solid var(--accent);
    box-shadow: 0 4px 18px rgba(0, 0, 0, 0.5);
    white-space: nowrap;
    font-size: 14px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  /* Nothing here may push the box wider (2r repair): the count and the length give way, with an
     ellipsis and the whole in their title; the kind and the times keep their room. */
  .info {
    height: 20px;
  }
  .info > span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .kind,
  .times {
    flex-shrink: 0;
  }
  .decide {
    height: 36px;
  }
  .more {
    height: 28px;
    gap: 6px;
  }
  /* Room for "123 of 456". */
  .where {
    flex: 0 1 10ch;
    color: var(--muted);
    font-size: 13px;
  }
  /* Room for "Range 999". */
  .kind {
    width: 84px;
    color: var(--good);
    font-weight: 600;
  }
  .kind.cut {
    color: var(--bad);
  }
  .times {
    font-size: 12px;
  }
  .len {
    flex: 1 1 0;
    text-align: right;
    color: var(--muted);
    font-size: 12px;
  }
  .all {
    flex: 1;
    text-align: center;
    color: var(--muted);
  }
  button {
    border-radius: 999px;
    padding: 3px 12px;
  }
  .decide button {
    height: 36px;
    width: 68px;
    padding: 0;
  }
  .decide .keep,
  .decide .remove {
    width: 120px;
    font-size: 15px;
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
  .now {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--muted);
    font-size: 12px;
  }
  .speeds {
    display: inline-flex;
    gap: 3px;
  }
  .speeds button {
    width: 40px;
    padding: 3px 0;
    font-size: 12px;
  }
  .speeds .on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .i {
    width: 34px;
    padding: 3px 0;
  }
  .done {
    width: 60px;
    padding: 3px 0;
  }
</style>
