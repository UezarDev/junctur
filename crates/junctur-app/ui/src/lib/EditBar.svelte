<script lang="ts">
  // Editing controls: cut or keep the selection (transcript words or a timeline stretch), undo and
  // redo, and the stretch under the playhead with buttons that nudge either end by a frame (shift:
  // ten). Every action is one edit the engine saves to edit.junctur at once.
  import * as api from "./api";
  import * as sel from "./selection";
  import { nudge, under } from "./nudge";
  import { timecode, toFrame } from "./time";

  let {
    player,
    edit,
    selection,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    selection: sel.Selection | null;
    onError: (message: string) => void;
  } = $props();

  const fps = $derived(edit?.fps ?? null);
  const frame = $derived(player?.frame ?? (fps && player?.position != null ? toFrame(player.position, fps) : null));
  // An unreadable edit list, an outside change, a backup made: said under the buttons.
  const message = $derived(edit?.error ?? edit?.notice ?? null);
  const stretch = $derived(edit && frame != null ? under(edit, frame) : null);

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
  <div class="bar">
    <span class="what">{sel.describe(selection, fps)}</span>
    <button disabled={!selection} onclick={() => selection && call(() => sel.apply(selection, false))} title="Cut the selection (Delete)"
      >Cut</button
    >
    <button disabled={!selection} onclick={() => selection && call(() => sel.apply(selection, true))} title="Keep the selection (K)"
      >Keep</button
    >
    <button disabled={!edit.canUndo} onclick={() => call(api.undo)} title="Undo (Ctrl+Z)">Undo</button>
    <button disabled={!edit.canRedo} onclick={() => call(api.redo)} title="Redo (Ctrl+Y)">Redo</button>
    {#if stretch && fps}
      <span class="range">
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
      </span>
    {/if}
    {#if message}
      <span class="message" class:bad={!!edit.error}>{message}</span>
    {/if}
  </div>
{/if}

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .what {
    min-width: 110px;
    color: var(--muted);
  }
  .range {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-left: auto;
  }
  .kind {
    margin-right: 4px;
    color: var(--good);
  }
  .kind.cut {
    color: var(--bad);
  }
  .n {
    padding: 2px 6px;
    font-size: 11px;
  }
  .message {
    flex-basis: 100%;
    color: var(--muted);
    font-size: 12px;
  }
  .message.bad {
    color: var(--bad);
  }
  .dash {
    color: var(--muted);
  }
  button {
    padding: 3px 9px;
  }
</style>
