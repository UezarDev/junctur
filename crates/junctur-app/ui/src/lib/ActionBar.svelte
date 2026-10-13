<script lang="ts">
  // Editing controls that float beside what they act on (2g, mock B), replacing 2c's fixed edit
  // row. With a selection (transcript words or a stretch of the lanes or timeline): what is
  // selected, Cut, Keep, Remember voice… (5b: name whoever says the selected words; their voice is
  // remembered for every recording), Undo and Redo, at the selection. With none: the stretch under
  // the playhead (a kept range or a cut) with buttons that nudge either end by a frame (shift: ten)
  // and cut or keep it whole, and Undo and Redo, at the playhead. Every action is one edit the
  // engine saves to edit.junctur at once. Its notices (an outside change, a backup made, the voice
  // result) show on a line above it. The keyboard does the same without it (Delete, K, Ctrl+Z/Y).
  import * as api from "./api";
  import * as sel from "./selection";
  import { nudge, under } from "./nudge";
  import { timecode, toFrame } from "./time";

  let {
    player,
    edit,
    selection,
    at,
    onSelect,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    selection: sel.Selection | null;
    /** Where to float: centred on `x` (window pixels), kept between `min` and `max`, its bottom at
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

  // Remembering a voice: the name field is open while `naming`; `said` is the engine's answer.
  let naming = $state(false);
  let name = $state("");
  let said = $state<string | null>(null);
  let known = $state<string[]>([]);
  async function startNaming() {
    naming = true;
    said = null;
    const fromVoices = await api.voices().then((v) => v.map((x) => x.name)).catch(() => [] as string[]);
    known = [...new Set([...fromVoices, ...(edit?.speakers ?? []).map((s) => s.name)])];
  }
  function focus(el: HTMLInputElement) {
    el.focus();
  }
  async function remember() {
    const n = name.trim();
    if (!n || selection?.kind !== "words") return;
    const { first, last } = selection;
    naming = false;
    try {
      said = await api.enrollVoice(first, last, n);
    } catch (e) {
      onError(String(e));
    }
  }

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

  // A new selection closes a naming left open for the old one.
  $effect(() => {
    void selection;
    naming = false;
  });
</script>

{#if edit?.editable}
  <div class="float" style:left="{left}px" style:bottom="{at.bottom}px" bind:clientWidth={width}>
    {#if message || said}
      <div class="message" class:bad={!!edit.error}>{said ?? message}</div>
    {/if}
    <div class="bar">
      {#if selection}
        <span class="what mono">{sel.describe(selection, fps)}</span>
        <button onclick={() => selection && call(() => sel.apply(selection, false))} title="Cut the selection (Delete)">Cut</button>
        <button onclick={() => selection && call(() => sel.apply(selection, true))} title="Keep the selection (K)">Keep</button>
        {#if naming}
          <span class="naming">
            <input
              use:focus
              list="junctur-voice-names"
              placeholder="Whose voice is this?"
              maxlength="60"
              bind:value={name}
              onkeydown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") remember();
                if (e.key === "Escape") naming = false;
              }}
            />
            <datalist id="junctur-voice-names">
              {#each known as k (k)}<option value={k}></option>{/each}
            </datalist>
            <button disabled={!name.trim()} onclick={remember}>Remember</button>
            <button onclick={() => (naming = false)}>Cancel</button>
          </span>
        {:else}
          <button
            disabled={selection.kind !== "words"}
            onclick={startNaming}
            title={selection.kind === "words"
              ? "Name whoever says these words (only one person, the more the better): their voice is remembered for every recording"
              : "Select words in the transcript to remember a voice"}>Remember voice…</button
          >
        {/if}
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
      {#if selection}
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
  .naming {
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }
  .naming input {
    width: 160px;
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
