<script lang="ts">
  // The transcript beside the video: kept words in order, cut runs folded, the word under the
  // playhead lit and followed. Rows have one fixed height and only those on screen (plus a few) are
  // in the DOM, so a 30 h transcript scrolls like a short one. An edit rebuilds the tokens
  // (transcript.ts, one pass over the words); a resize or a fold toggle only repacks them into rows.
  // Where the speaker changes a name row comes first; click the name to rename the speaker (saved
  // to edit.junctur at once).
  import { onMount } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import * as api from "./api";
  import * as sel from "./selection";
  import { toSeconds } from "./time";
  import { pack, stamp, tokens, wordAt, type Stretch, type Token, type Words } from "./transcript";

  let {
    words,
    loading,
    player,
    edit,
    selection,
    onSelect,
    onError,
  }: {
    words: Words | null;
    loading: boolean;
    player: api.PlayerState | null;
    edit: api.EditState | null;
    selection: sel.Selection | null;
    onSelect: (s: sel.Selection | null) => void;
    onError: (message: string) => void;
  } = $props();

  const ROW = 24;
  const GUTTER = 58;
  const PAD = 12;
  // Rows rendered beyond the visible ones, so a fast scroll does not show blanks.
  const OVERSCAN = 12;
  // After the user scrolls, the list stops following the playhead for this long.
  const FOLLOW_PAUSE_MS = 4000;
  // A window drag resizes every frame; rows are repacked once it settles.
  const RESIZE_SETTLE_MS = 120;
  const FONT = '14px "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif';
  const SMALL = '12px "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif';
  const SPACE = 4;

  let scroller: HTMLDivElement;
  let width = $state(0);
  let height = $state(0);
  let top = $state(0);
  const open = new SvelteSet<number>();

  // A new transcript (another recording) starts with every fold closed.
  $effect(() => {
    void words;
    open.clear();
  });

  // Text widths measured once per distinct string (speech repeats words a lot).
  const ctx = document.createElement("canvas").getContext("2d")!;
  const cache = new Map<string, number>();
  function measure(text: string, font: string): number {
    const key = font === FONT ? text : `\u0001${text}`;
    let w = cache.get(key);
    if (w === undefined) {
      ctx.font = font;
      w = ctx.measureText(text).width;
      cache.set(key, w);
    }
    return w;
  }

  function seconds(frames: Stretch): number {
    return edit?.fps ? toSeconds(frames.end - frames.start, edit.fps) : 0;
  }

  function foldLabel(t: Extract<Token, { kind: "cut" }>, isOpen: boolean): string {
    const n = t.last - t.first + 1;
    return `${isOpen ? "▾" : "▸"} cut ${seconds(t.frames).toFixed(1)} s, ${n} word${n === 1 ? "" : "s"}`;
  }

  function markWidth(t: Token): number {
    switch (t.kind) {
      case "word":
      case "cutword":
        return 0; // measured per transcript, in `wordWidth`
      case "cut": {
        // The wider of the two labels (open or closed), the padding, and the keep button.
        const label = Math.max(measure(foldLabel(t, true), SMALL), measure(foldLabel(t, false), SMALL));
        return label + measure("keep", SMALL) + 34 + SPACE;
      }
      case "gap":
        return 10 + SPACE;
    }
  }

  // Only a slow step is logged (test mode reads the log; a 30 h transcript is the case to watch).
  const SLOW_MS = 50;
  function timed<T>(what: string, f: () => T): T {
    const t0 = performance.now();
    const out = f();
    const ms = performance.now() - t0;
    if (ms > SLOW_MS) void api.uiLog(`slow transcript ${what}: ${ms.toFixed(0)} ms`).catch(() => {});
    return out;
  }

  // Per transcript: each word's width.
  const wordWidth = $derived(
    words ? timed("widths", () => Float64Array.from(words!.text, (t) => measure(t, FONT) + SPACE)) : null,
  );
  // Per edit: which words are kept, the folds.
  const toks = $derived(
    words && wordWidth ? timed("tokens", () => tokens(words!, wordWidth!, edit, markWidth)) : null,
  );
  // Per resize or fold toggle: rows only.
  const laid = $derived(
    toks && words && width > 0
      ? timed("rows", () =>
          pack(toks, words!.text.length, open, Math.max(80, width - GUTTER - 2 * PAD), words!.speaker),
        )
      : null,
  );

  // Speakers: names from the edit list, a display colour by id (the caption colours are a later
  // part of 5a; these only tell the speakers apart here).
  const PALETTE = ["#f2b33d", "#5fb3f2", "#8bd17c", "#e58bd8", "#f28b5f", "#7fd8d0", "#c9a3f2", "#d8d36f"];
  const names = $derived(new Map((edit?.speakers ?? []).map((s) => [s.id, s.name])));
  const colour = (id: number) => PALETTE[(id - 1) % PALETTE.length];
  const nameOf = (id: number) => names.get(id) ?? `Speaker ${id}`;

  // Renaming: one name row at a time turns into an input; Enter or leaving it saves, Escape drops.
  let renaming = $state<number | null>(null);
  let draft = $state("");
  function startRename(id: number) {
    renaming = id;
    draft = nameOf(id);
  }
  function focusInput(el: HTMLInputElement) {
    el.focus();
    el.select();
  }
  function finishRename(save: boolean) {
    const id = renaming;
    renaming = null;
    if (!save || id == null) return;
    const name = draft.trim();
    if (name && name !== nameOf(id)) call(() => api.renameSpeaker(id, name));
  }

  const first = $derived(Math.max(0, Math.floor(top / ROW) - OVERSCAN));
  const visible = $derived.by(() => {
    if (!laid) return [];
    const last = Math.min(laid.rows.length, Math.ceil((top + height) / ROW) + OVERSCAN);
    const out = [];
    for (let i = first; i < last; i++) out.push({ index: i, row: laid.rows[i] });
    return out;
  });

  const current = $derived(words && player?.position != null ? wordAt(words, player.position * 1000) : -1);
  const chosen = $derived(selection?.kind === "words" ? selection : null);

  // Following the playhead: scroll so the current word's row stays in view, unless the user has
  // scrolled lately. Our own scrolls are told apart by the position they set.
  let pausedUntil = 0;
  let ours = -1;
  $effect(() => {
    if (!laid || current < 0 || !scroller) return;
    const row = laid.rowOf[current];
    if (performance.now() < pausedUntil) return;
    const y = row * ROW;
    if (y < top + ROW || y > top + height - 3 * ROW) {
      ours = Math.max(0, Math.round(y - height / 3));
      scroller.scrollTop = ours;
    }
  });

  function onScroll() {
    top = scroller.scrollTop;
    if (ours < 0 || Math.abs(top - ours) > 1) pausedUntil = performance.now() + FOLLOW_PAUSE_MS;
    ours = -1;
  }

  async function call(f: () => Promise<unknown>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }

  // Selecting: press on a word, drag across others, release. A press and release on one word
  // without moving seeks there (and selects it); shift extends the selection from its anchor.
  // `pressed` is the one drag in progress; `anchor` outlives it, for shift-clicks.
  let anchor = -1;
  let pressed = false;
  let moved = false;
  function wordUnder(target: EventTarget | Element | null): number {
    const el = (target as HTMLElement | null)?.closest?.<HTMLElement>("[data-i]");
    return el ? Number(el.dataset.i) : -1;
  }

  function down(e: PointerEvent) {
    pressed = false;
    const i = wordUnder(e.target);
    if (i < 0 || e.button !== 0) return;
    e.preventDefault();
    if (e.shiftKey && chosen) {
      // The anchor only counts while it is an end of the current selection (the timeline may
      // have replaced it).
      const from = anchor === chosen.first || anchor === chosen.last ? anchor : chosen.first;
      anchor = from;
      onSelect(sel.words(from, i));
      moved = true;
    } else {
      anchor = i;
      moved = false;
      onSelect(sel.words(i, i));
    }
    pressed = true;
    scroller.setPointerCapture(e.pointerId);
  }

  function move(e: PointerEvent) {
    if (!pressed || !(e.buttons & 1)) return;
    // Captured: the target is the scroller, so find the word under the pointer instead.
    const i = wordUnder(document.elementFromPoint(e.clientX, e.clientY));
    if (i >= 0 && (i !== anchor || moved)) {
      moved = true;
      onSelect(sel.words(anchor, i));
    }
  }

  function up() {
    if (!pressed) return;
    pressed = false;
    if (!moved && words && anchor >= 0) {
      const i = anchor;
      call(() => api.seek(words!.start[i] / 1000, true));
    }
    moved = false;
  }

  function toggle(t: Extract<Token, { kind: "cut" }>) {
    if (open.has(t.first)) open.delete(t.first);
    else open.add(t.first);
  }

  function restore(frames: Stretch) {
    call(() => api.editFrames(frames.start, frames.end, true));
  }

  onMount(() => {
    let timer = 0;
    const ro = new ResizeObserver(() => {
      height = scroller.clientHeight;
      clearTimeout(timer);
      // The first size at once, later ones once the resize settles.
      if (width === 0) width = scroller.clientWidth;
      else timer = window.setTimeout(() => (width = scroller.clientWidth), RESIZE_SETTLE_MS);
    });
    ro.observe(scroller);
    return () => {
      clearTimeout(timer);
      ro.disconnect();
    };
  });
</script>

<section class="panel">
  <header>
    <span>Transcript</span>
    {#if words}
      <span class="muted">{words.text.length.toLocaleString()} words</span>
    {/if}
  </header>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="scroll"
    bind:this={scroller}
    onscroll={onScroll}
    onwheel={() => (pausedUntil = performance.now() + FOLLOW_PAUSE_MS)}
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
  >
    {#if loading}
      <p class="note">Reading the transcript…</p>
    {:else if !words}
      <p class="note">No transcript for this recording yet. Run the pipeline on it to edit by text.</p>
    {:else if laid}
      <div class="spacer" style:height="{laid.rows.length * ROW}px">
        {#each visible as { index, row } (index)}
          <div class="row" style:top="{index * ROW}px" style:padding-left="{GUTTER}px">
            {#if row.speaker != null}
              {@const id = row.speaker}
              {#if renaming === id}
                <input
                  class="rename"
                  maxlength="60"
                  bind:value={draft}
                  use:focusInput
                  onpointerdown={(e) => e.stopPropagation()}
                  onkeydown={(e) => {
                    e.stopPropagation();
                    if (e.key === "Enter") finishRename(true);
                    else if (e.key === "Escape") finishRename(false);
                  }}
                  onblur={() => finishRename(true)}
                  aria-label="Speaker name"
                />
              {:else}
                <button
                  class="speaker"
                  style:color={colour(id)}
                  onpointerdown={(e) => e.stopPropagation()}
                  onclick={() => startRename(id)}
                  title="Rename this speaker">{nameOf(id)}</button
                >
              {/if}
            {/if}
            {#if row.para != null}<span class="time">{stamp(row.para)}</span>{/if}
            {#each row.tokens as k (k)}
              {@const t = toks!.list[k]}
              {#if t.kind === "word"}
                <span
                  class="w"
                  class:cur={t.i === current}
                  class:sel={chosen && t.i >= chosen.first && t.i <= chosen.last}
                  data-i={t.i}>{words.text[t.i]}</span
                >
              {:else if t.kind === "cutword"}
                <span
                  class="w cutword"
                  class:cur={t.i === current}
                  class:sel={chosen && t.i >= chosen.first && t.i <= chosen.last}
                  data-i={t.i}>{words.text[t.i]}</span
                >
              {:else if t.kind === "cut"}
                <span class="fold">
                  <button class="label" onclick={() => toggle(t)} title="Show or hide the cut words"
                    >{foldLabel(t, open.has(t.first))}</button
                  ><button class="keep" onclick={() => restore(t.frames)} title="Keep this cut stretch again"
                    >keep</button
                  >
                </span>
              {:else}
                <button
                  class="gap"
                  onclick={() => restore(t.frames)}
                  title="Cut {seconds(t.frames).toFixed(1)} s with no words; click to keep it"
                  aria-label="Keep this cut"
                ></button>
              {/if}
            {/each}
          </div>
        {/each}
      </div>
    {/if}
  </div>
</section>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--panel-solid);
    border-left: 1px solid var(--line);
  }
  header {
    display: flex;
    justify-content: space-between;
    padding: 10px 12px 8px;
    font-weight: 600;
    border-bottom: 1px solid var(--line);
  }
  .muted {
    color: var(--muted);
    font-weight: 400;
  }
  .scroll {
    position: relative;
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
  }
  .note {
    margin: 16px 12px;
    color: var(--muted);
  }
  .spacer {
    position: relative;
  }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    height: 24px;
    line-height: 24px;
    padding-right: 12px;
    white-space: nowrap;
    overflow: hidden;
    font: 14px "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif;
  }
  .time {
    position: absolute;
    left: 12px;
    color: var(--muted);
    font: 11px "Cascadia Mono", Consolas, monospace;
  }
  .w {
    margin-right: 4px;
    border-radius: 3px;
    cursor: pointer;
  }
  .w:hover {
    background: rgba(255, 255, 255, 0.08);
  }
  .w.sel {
    background: rgba(242, 179, 61, 0.28);
  }
  .w.cur {
    color: #101114;
    background: var(--accent);
  }
  .cutword {
    color: var(--muted);
    text-decoration: line-through;
  }
  .fold {
    display: inline-flex;
    margin-right: 4px;
    vertical-align: middle;
  }
  .fold button {
    font-size: 12px;
    line-height: 16px;
    padding: 1px 6px;
    color: var(--muted);
    background: rgba(239, 111, 94, 0.12);
    border: 1px solid rgba(239, 111, 94, 0.35);
  }
  .fold .label {
    border-radius: 4px 0 0 4px;
  }
  .fold .keep {
    border-radius: 0 4px 4px 0;
    border-left: none;
  }
  .gap {
    display: inline-block;
    width: 4px;
    height: 14px;
    margin: 0 7px 0 3px;
    padding: 0;
    vertical-align: middle;
    border: none;
    border-radius: 2px;
    background: rgba(239, 111, 94, 0.45);
  }
  .gap:hover {
    background: var(--bad);
  }
  .speaker {
    padding: 0 4px;
    font-size: 12px;
    font-weight: 600;
    line-height: 20px;
    background: none;
    border: none;
    border-radius: 3px;
    cursor: text;
  }
  .speaker:hover {
    background: rgba(255, 255, 255, 0.08);
  }
  .rename {
    width: 220px;
    height: 20px;
    font-size: 12px;
    font-weight: 600;
  }
</style>
