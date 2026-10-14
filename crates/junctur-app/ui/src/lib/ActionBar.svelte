<script lang="ts">
  // Editing controls on the picture: one fixed-size box (layout.ts `TOOLS`) in its bottom-right
  // corner, clear of the captions. Every state draws in the same two rows, so no text moves it.
  import * as api from "./api";
  import * as sel from "./selection";
  import { nudge, under } from "./nudge";
  import { timecode, toFrame } from "./time";
  import { TOOLS } from "./layout";

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
    /** The Transcript tab is showing, with its own bar for a selection of words. */
    wordsShown?: boolean;
    /** Where the box's top-left goes (layout.ts `floatAt`). */
    at: { left: number; top: number; scale: number };
    onSelect: (s: sel.Selection | null) => void;
    onError: (message: string) => void;
  } = $props();

  const fps = $derived(edit?.fps ?? null);
  const frame = $derived(player?.frame ?? (fps && player?.position != null ? toFrame(player.position, fps) : null));
  const message = $derived(edit?.error ?? edit?.notice ?? null);
  const stretch = $derived(edit && frame != null && !selection ? under(edit, frame) : null);

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
    // The words beside the edge move with it.
    const from = edge === "start" ? stretch.start : stretch.end;
    if (step) call(() => api.moveEdge(step, from));
  }
</script>

{#if edit?.editable}
  <div class="float" style:left="{at.left}px" style:top="{at.top}px" style:width="{TOOLS.w}px" style:height="{TOOLS.h}px" style:transform={at.scale < 1 ? `scale(${at.scale})` : undefined}>
    {#if message}
      <div class="message" class:bad={!!edit.error}>{message}</div>
    {/if}
    <div class="row">
      {#if mine}
        <span class="what" title={sel.describe(mine, fps)}>{sel.describe(mine, fps)}</span>
      {:else if selection}
        <span class="what">Selected in the Transcript</span>
      {:else if stretch && fps}
        <span class="what kind" class:cut={!stretch.kept}>{stretch.kept ? `Range ${stretch.index}` : "Cut"}</span>
        <button
          class="whole"
          onclick={() => call(() => api.editFrames(stretch!.start, stretch!.end, !stretch!.kept))}
          title={stretch.kept ? "Cut this whole range" : "Keep this whole cut"}>{stretch.kept ? "Cut range" : "Keep cut"}</button
        >
      {:else}
        <span class="what"></span>
      {/if}
      <button class="i" disabled={!edit.canUndo} onclick={() => call(api.undo)} title="Undo (Ctrl+Z)" aria-label="Undo">↶</button>
      <button class="i" disabled={!edit.canRedo} onclick={() => call(api.redo)} title="Redo (Ctrl+Y)" aria-label="Redo">↷</button>
    </div>
    <div class="row">
      {#if mine}
        {@const m = mine}
        <button class="act" onclick={() => call(() => sel.apply(m, false))} title="Cut the selection (Delete)">Cut</button>
        <button class="act" onclick={() => call(() => sel.apply(m, true))} title="Keep the selection (K)">Keep</button>
        <span class="gap"></span>
        <button class="i x" onclick={() => onSelect(null)} title="Clear the selection (Esc)" aria-label="Clear the selection">×</button>
      {:else if stretch && fps}
        <button class="n" onclick={(e) => move("start", -1, e)} title="Start one frame earlier (shift: 10)">◀</button>
        <span class="tc mono">{timecode(stretch.start, fps)}</span>
        <button class="n" onclick={(e) => move("start", 1, e)} title="Start one frame later (shift: 10)">▶</button>
        <span class="dash">–</span>
        <button class="n" onclick={(e) => move("end", -1, e)} title="End one frame earlier (shift: 10)">◀</button>
        <span class="tc mono">{timecode(stretch.end, fps)}</span>
        <button class="n" onclick={(e) => move("end", 1, e)} title="End one frame later (shift: 10)">▶</button>
      {/if}
    </div>
  </div>
{/if}

<style>
  /* Drawn at exactly TOOLS' size (border and padding inside), which floatAt places by. */
  .float {
    position: absolute;
    z-index: 5;
    /* floatAt's scale, in a video area smaller than the box, from the corner it places. */
    transform-origin: top left;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 6px;
    padding: 8px 12px;
    border-radius: 16px;
    background: rgba(28, 29, 34, 0.94);
    border: 1px solid rgba(255, 255, 255, 0.16);
    box-shadow: 0 4px 18px rgba(0, 0, 0, 0.5);
    white-space: nowrap;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
  }
  .what {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--muted);
    font-size: 13px;
  }
  .kind {
    color: var(--good);
    font-weight: 600;
  }
  .kind.cut {
    color: var(--bad);
  }
  button {
    border-radius: 999px;
    padding: 3px 10px;
  }
  /* As wide as "Keep cut" / "Cut range", whichever shows. */
  .whole {
    width: 92px;
  }
  .act {
    width: 80px;
    font-weight: 600;
  }
  .gap {
    flex: 1;
  }
  .n {
    width: 26px;
    padding: 2px 0;
    font-size: 11px;
  }
  .i {
    width: 34px;
    padding: 2px 0;
  }
  .x {
    color: var(--muted);
  }
  .dash {
    color: var(--muted);
  }
  /* 11 figures: room for 10:00:00:00, so the hours never move the steppers. */
  .tc {
    width: 11ch;
    text-align: center;
    font-size: 12px;
  }
  .message {
    position: absolute;
    right: 0;
    bottom: calc(100% + 6px);
    max-width: 100%;
    box-sizing: border-box;
    padding: 4px 10px;
    border-radius: 12px;
    background: rgba(16, 17, 20, 0.92);
    color: var(--muted);
    font-size: 12px;
    white-space: normal;
  }
  .message.bad {
    color: var(--bad);
  }
</style>
