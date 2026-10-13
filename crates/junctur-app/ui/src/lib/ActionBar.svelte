<script lang="ts">
  // Editing controls over the lanes (2g, mock B), replacing 2c's fixed edit row. They sit in one
  // place, centred over the lanes (2n; 2g floated them beside the selection or the playhead, so
  // they slid along while playing). With a selection (a stretch of the lanes or timeline, or
  // transcript words while the Transcript tab is not showing): what is selected, Cut, Keep, Undo
  // and Redo. Words selected with the Transcript tab showing have their controls at its bottom
  // (WordBar, 2p, with the one-click speakers that replaced 5b's Remember voice… here), so this bar
  // then holds Undo and Redo only and the two never show the same thing twice. With none: the stretch
  // under the playhead (a kept range or a cut) with buttons that nudge either end by a frame
  // (shift: ten) and cut or keep it whole, and Undo and Redo. Every action is one edit the
  // engine saves to edit.junctur at once. Its notices (an outside change, a backup made) show on a
  // line above it. The keyboard does the same without it (Delete, K, Ctrl+Z/Y).
  import * as api from "./api";
  import * as sel from "./selection";
  import { nudge, under } from "./nudge";
  import { timecode, toFrame } from "./time";

  let {
    player,
    edit,
    selection,
    wordsShown = false,
    at,
    onSelect,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    selection: sel.Selection | null;
    /** The Transcript tab is showing, with its own bar for a selection of words (2p). */
    wordsShown?: boolean;
    /** Where to sit: centred on `x` (window pixels), kept between `min` and `max`, its bottom at
     * `bottom`. */
    at: { x: number; min: number; max: number; bottom: number };
    onSelect: (s: sel.Selection | null) => void;
    onError: (message: string) => void;
  } = $props();

  let width = $state(0);
  const fps = $derived(edit?.fps ?? null);
  const frame = $derived(player?.frame ?? (fps && player?.position != null ? toFrame(player.position, fps) : null));
  const message = $derived(edit?.error ?? edit?.notice ?? null);
  const stretch = $derived(edit && frame != null && !selection ? under(edit, frame) : null);
  const left = $derived(Math.round(Math.min(Math.max(at.x - width / 2, at.min), Math.max(at.min, at.max - width))));

  // The selection this bar acts on: words are the Transcript's own while it shows them.
  const mine = $derived(selection?.kind === "words" && wordsShown ? null : selection);

  async function call(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }

  function move(edge: "start" | "end", dir: number, e: MouseEvent) {
    if (!stretch || !edit) return;
    const step = nudge(stretch, edge, dir * (e.shiftKey ? 10 : 1), edit.totalFrames);
    if (step) call(() => api.editFrames(step.start, step.end, step.keep));
  }
</script>

{#if edit?.editable}
  <div class="float" style:left="{left}px" style:bottom="{at.bottom}px" bind:clientWidth={width}>
    {#if message}
      <div class="message" class:bad={!!edit.error}>{message}</div>
    {/if}
    <div class="bar">
      {#if mine}
        {@const m = mine}
        <span class="what mono">{sel.describe(m, fps)}</span>
        <button onclick={() => call(() => sel.apply(m, false))} title="Cut the selection (Delete)">Cut</button>
        <button onclick={() => call(() => sel.apply(m, true))} title="Keep the selection (K)">Keep</button>
      {:else if stretch && fps}
        <span class="kind" class:cut={!stretch.kept}>{stretch.kept ? `Range ${stretch.index}` : "Cut"}</span>
        <button class="n" onclick={(e) => move("start", -1, e)} title="Start one frame earlier (shift: 10)">◀</button>
        <span class="mono">{timecode(stretch.start, fps)}</span>
        <button class="n" onclick={(e) => move("start", 1, e)} title="Start one frame later (shift: 10)">▶</button>
        <span class="dash">–</span>
        <button class="n" onclick={(e) => move("end", -1, e)} title="End one frame earlier (shift: 10)">◀</button>
        <span class="mono">{timecode(stretch.end, fps)}</span>
        <button class="n" onclick={(e) => move("end", 1, e)} title="End one frame later (shift: 10)">▶</button>
        <button
          onclick={() => call(() => api.editFrames(stretch!.start, stretch!.end, !stretch!.kept))}
          title={stretch.kept ? "Cut this whole range" : "Keep this whole cut"}>{stretch.kept ? "Cut range" : "Keep cut"}</button
        >
      {/if}
      <button class="i" disabled={!edit.canUndo} onclick={() => call(api.undo)} title="Undo (Ctrl+Z)" aria-label="Undo">↶</button>
      <button class="i" disabled={!edit.canRedo} onclick={() => call(api.redo)} title="Redo (Ctrl+Y)" aria-label="Redo">↷</button>
      {#if mine}
        <button class="i x" onclick={() => onSelect(null)} title="Clear the selection (Esc)" aria-label="Clear the selection">×</button>
      {/if}
    </div>
  </div>
{/if}

<style>
  .float {
    position: absolute;
    z-index: 5;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    max-width: 90vw;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 6px;
    border-radius: 8px;
    background: rgba(28, 29, 34, 0.96);
    border: 1px solid rgba(255, 255, 255, 0.16);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.45);
    white-space: nowrap;
  }
  .what {
    padding: 0 4px;
    color: var(--muted);
    font-size: 12px;
  }
  .kind {
    padding: 0 4px;
    color: var(--good);
  }
  .kind.cut {
    color: var(--bad);
  }
  .n {
    padding: 2px 6px;
    font-size: 11px;
  }
  .i {
    padding: 2px 8px;
  }
  .x {
    color: var(--muted);
  }
  .dash {
    color: var(--muted);
  }
  .mono {
    font-size: 12px;
  }
  button {
    padding: 3px 9px;
  }
  .message {
    max-width: 520px;
    padding: 3px 8px;
    border-radius: 6px;
    background: rgba(16, 17, 20, 0.92);
    color: var(--muted);
    font-size: 12px;
    white-space: normal;
  }
  .message.bad {
    color: var(--bad);
  }
</style>
