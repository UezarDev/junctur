<script lang="ts">
  // Editing controls: cut or keep the selection (transcript words or a timeline stretch), undo and
  // redo, and the stretch under the playhead with buttons that nudge either end by a frame (shift:
  // ten). Every action is one edit the engine saves to edit.junctur at once. With words selected,
  // "Remember voice…" (5b) saves the voice of whoever says them under a name, for every recording,
  // and names this recording's speakers with it.
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
        disabled={selection?.kind !== "words"}
        onclick={startNaming}
        title="Select words only one person says (the more, the better), then name them: their voice is remembered for every recording"
        >Remember voice…</button
      >
    {/if}
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
    {#if said}
      <span class="message">{said}</span>
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
</style>
