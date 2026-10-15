<script lang="ts">
  // What a selection of transcript words can do: Cut, Keep, Fix text, one-click speakers (each
  // remembers that voice) and Correct. Apart from TranscriptPanel, which is the virtual word list.
  // Shown, as a hint, with nothing selected too: its rows stay laid out but hidden, so selecting
  // never changes the panel's height under a double-click.
  import { onDestroy, untrack } from "svelte";
  import * as api from "./api";
  import * as sel from "./selection";
  import { shownText, wordRef, type Words } from "./transcript";

  let {
    edit,
    words,
    selection,
    onSelect,
    onError,
    checks = false,
    onLooksRight = null,
    retyping = $bindable(false),
    onFixed,
    onCorrect,
    analysing = false,
  }: {
    edit: api.EditState | null;
    words: Words;
    /** A selection of words, or null for the hint. */
    selection: sel.Selection | null;
    onSelect: (s: sel.Selection | null) => void;
    onError: (message: string) => void;
    /** The transcript has flagged words: Looks right is in the bar, greyed unless it applies. */
    checks?: boolean;
    /** The selection is one flagged word: confirms it as heard and goes to the next; else null. */
    onLooksRight?: (() => void) | null;
    /** The Fix text field is open (the panel opens it on a double-click). */
    retyping?: boolean;
    /** What a fix did besides itself: the word list's new entries. */
    onFixed: (r: api.Fixed) => void;
    /** The Correct button: opens the pop-up. */
    onCorrect: () => Promise<void>;
    /** The recording is being analysed: Correct waits until it is done. */
    analysing?: boolean;
  } = $props();

  // A speaker the analysis named "Speaker 3" has no name set yet: remembering a voice under that
  // label would make a person called "Speaker 3" in every recording, so those are not offered.
  const UNNAMED = /^Speaker \d+$/;

  let voices = $state<api.Voice[]>([]);
  let busy = $state(false);
  let said = $state<string | null>(null);
  let naming = $state(false);
  let correcting = $state(false);
  let name = $state("");

  async function loadVoices() {
    voices = await api.voices().catch(() => voices);
  }
  // Asked again when the speakers change (an enrolment, a rename, a forget elsewhere).
  $effect(() => {
    void edit?.speakersKey;
    void loadVoices();
  });

  // The recording's named speakers first (their caption colours), then remembered voices not
  // among them; one button per name, whatever its case.
  const people = $derived.by(() => {
    const out: { name: string; color: string }[] = [];
    const seen = new Set<string>();
    const add = (n: string, color: string) => {
      const key = n.trim().toLowerCase();
      if (!key || seen.has(key) || UNNAMED.test(n.trim())) return;
      seen.add(key);
      out.push({ name: n.trim(), color });
    };
    for (const s of edit?.speakers ?? []) add(s.name, s.color);
    for (const v of voices) add(v.name, v.color);
    return out;
  });

  const runs = $derived(sel.pairs(selection));

  // Fix text, for the stretch it was opened on. Leaving it any way saves what was typed; Escape and
  // the field's own × leave the text as it was. In the bar, as a virtual list's row has one height.
  // A field left as it opened saves nothing, and every save (Enter's too) goes to the stretch it
  // opened on, named by its end words as they were then, which the engine checks (5ae: a save by
  // bare index wrote onto whatever words held it by then).
  let draft = $state("");
  let shown = "";
  let openFor: string | null = null;
  // The stretch the open field is for (plain: read by the teardown, after the props have moved on).
  let openRun: { first: number; last: number } | null = null;
  let openEnds: [api.WordRef, api.WordRef] | null = null;
  let saving = $state(false);
  const one = $derived(runs.length === 1 ? { first: runs[0][0], last: runs[0][1] } : null);
  $effect(() => {
    const at = one ? `${one.first}-${one.last}` : null;
    if (!retyping) {
      // Closed from outside (the header's Fixes, the selection gone): saved as any leaving is.
      // Escape and a save on its way have let go of `openRun` already, so they save nothing here.
      untrack(leave);
    } else if (openFor == null && at) {
      openFor = at;
      openRun = one;
      openEnds = [wordRef(words, one!.first), wordRef(words, one!.last)];
      shown = shownText(words, one!);
      draft = shown;
    } else if (openFor !== at || !at) {
      untrack(leave);
      retyping = false;
    }
  });

  /** A failed save's message for a field already closed: what was typed, so it is not lost. */
  const unsaved = (e: unknown, text: string) => `${String(e)} (not saved: "${text}")`;

  /** Saves what the open field holds for its own stretch, if it changed, and forgets the field. */
  function leave() {
    const run = openRun;
    openRun = null;
    openFor = null;
    if (!run || !openEnds || draft.trim() === shown.trim()) return;
    const text = draft;
    api.fixWords(run.first, run.last, openEnds, text).then(onFixed, (e) => onError(unsaved(e, text)));
  }
  onDestroy(leave);

  async function saveText() {
    const run = openRun;
    const ends = openEnds;
    if (!run || !ends || saving) return;
    if (draft.trim() === shown.trim()) {
      retyping = false;
      return;
    }
    saving = true;
    const text = draft;
    // Taken before the wait, so a blur or a new selection meanwhile does not save it a second time
    // through `leave`; given back if the save fails, so leaving can still save what was typed.
    openRun = null;
    try {
      onFixed(await api.fixWords(run.first, run.last, ends, text));
      retyping = false;
    } catch (e) {
      // The field still open on these words keeps the text. Closed meanwhile (another selection):
      // the text would be gone, so the message carries it.
      if (retyping && openFor === `${run.first}-${run.last}`) {
        openRun = run;
        onError(String(e));
      } else onError(unsaved(e, text));
    } finally {
      saving = false;
    }
  }

  // Selected once the bound value is in: selecting at mount, before it, left the caret at the end.
  function focusAll(el: HTMLInputElement) {
    el.focus();
    requestAnimationFrame(() => el.select());
  }

  // Another selection: the last answer was about the words before. An open "New name…" field
  // stays, since Ctrl+clicking more of someone's lines while naming them is the point; it goes
  // when the selection clears.
  $effect(() => {
    said = null;
    if (!selection) {
      naming = false;
      name = "";
    }
  });

  async function call(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }

  async function remember(who: string) {
    const n = who.trim();
    if (!n || busy || !runs.length) return;
    busy = true;
    said = null;
    naming = false;
    try {
      said = await api.enrollVoice(runs, n);
      name = "";
      await loadVoices();
    } catch (e) {
      onError(String(e));
    } finally {
      busy = false;
    }
  }

  function focus(el: HTMLInputElement) {
    el.focus();
  }
</script>

<div class="wordbar">
  <div class="body" class:idle={!selection}>
  {#if !selection}
    <div class="hint">Select words to cut, keep or fix them, or to say who says them</div>
  {/if}
  {#if said}
    <div class="said">{said}</div>
  {/if}
  <div class="rows" inert={!selection}>
  <div class="row">
    <span class="what mono">{sel.describe(selection, edit?.fps ?? null)}</span>
    <button onclick={() => selection && call(() => sel.apply(selection, false))} title="Cut every selected stretch (Delete)">Cut</button>
    <button onclick={() => selection && call(() => sel.apply(selection, true))} title="Keep every selected stretch (K)">Keep</button>
    <button
      disabled={!one}
      onclick={() => {
        // Closing the field by its button saves what was typed, as leaving it does.
        if (retyping) leave();
        retyping = !retyping;
      }}
      title={one
        ? "Retype these words as one text: shown in the transcript and the captions in their place (F2; double-click a selection does this too)"
        : "Select one stretch of words to fix its text: one text cannot go in several places"}>Fix text</button
    >
    {#if checks}
      <!-- Greyed rather than left out, as the header's ‹ › are, so the row does not reflow. -->
      <button
        disabled={!onLooksRight}
        onclick={() => onLooksRight?.()}
        title={onLooksRight
          ? "Whisper was unsure of this word but it is right as heard: stop flagging it and go to the next"
          : "Select one underlined word to keep it as heard"}>Looks right</button
      >
    {/if}
    <span class="grow"></span>
    <button class="x" onclick={() => onSelect(null)} title="Clear the selection (Esc)" aria-label="Clear the selection">×</button>
  </div>
  {#if retyping && one}
    <div class="row retype">
      <input
        use:focusAll
        maxlength="1000"
        placeholder="What these words say (empty takes the fix away)"
        bind:value={draft}
        disabled={saving}
        onkeydown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") void saveText();
          if (e.key === "Escape") {
            openRun = null;
            retyping = false;
          }
        }}
        aria-label="What the selected words say"
      />
      <button disabled={saving} onclick={() => void saveText()}>Save</button>
      <button
        class="x"
        onclick={() => {
          openRun = null;
          retyping = false;
        }}
        title="Leave the text as it was (Esc)"
        aria-label="Cancel">×</button
      >
    </div>
  {/if}
  <div class="row people" aria-label="Who says these words">
    <span class="label">{busy ? "Remembering…" : "Who says this:"}</span>
    {#each people as p (p.name)}
      <button
        class="person"
        style:--c={p.color}
        disabled={busy}
        onclick={() => remember(p.name)}
        title="These words are {p.name}'s: remember their voice from every selected stretch">{p.name}</button
      >
    {/each}
    {#if naming}
      <span class="naming">
        <input
          use:focus
          placeholder="Whose voice is this?"
          maxlength="60"
          bind:value={name}
          onkeydown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") remember(name);
            if (e.key === "Escape") naming = false;
          }}
        />
        <button disabled={!name.trim() || busy} onclick={() => remember(name)}>Remember</button>
      </span>
    {:else}
      <button
        class="new"
        disabled={busy}
        onclick={() => {
          naming = true;
          said = null;
        }}
        title="Someone not listed yet: name whoever says these words (only one person, the more the better)">New name…</button
      >
    {/if}
  </div>
  </div>
  </div>
  <!-- Outside the rows, so it is there with nothing selected too: it works on the whole
       recording or on the selection, the timeline's included. -->
  <button
    class="correct"
    disabled={busy || correcting || analysing}
    onclick={async () => {
      correcting = true;
      try {
        await onCorrect();
      } finally {
        correcting = false;
      }
    }}
    title={analysing
      ? "Analysing this recording: Correct waits until it is done"
      : "Analyse this recording again, part by part: the words, the speakers or the cut, keeping what you choose of yours, over the selection or the whole timeline"}
    >Correct…</button
  >
</div>

<style>
  .wordbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px 10px;
    border-top: 1px solid var(--line);
    background: var(--panel-solid);
  }
  .body {
    position: relative;
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .idle > .rows {
    visibility: hidden;
  }
  .hint {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    color: var(--muted);
    font-size: 12px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .grow {
    flex: 1;
  }
  .what {
    color: var(--muted);
    font-size: 12px;
  }
  .label {
    color: var(--muted);
    font-size: 12px;
  }
  .person {
    padding: 2px 9px;
    font-size: 12px;
    font-weight: 600;
    color: var(--c);
    border: 1px solid color-mix(in srgb, var(--c) 55%, transparent);
    background: color-mix(in srgb, var(--c) 12%, transparent);
  }
  .person:hover:not(:disabled) {
    background: color-mix(in srgb, var(--c) 24%, transparent);
  }
  .new {
    padding: 2px 9px;
    font-size: 12px;
  }
  /* The bar's one big button, beside its rows: the way to analyse the recording again. */
  .wordbar > .correct {
    flex: none;
    align-self: stretch;
    padding: 6px 16px;
    font-size: 14px;
    font-weight: 600;
    border-color: var(--accent);
    color: var(--accent);
  }
  .naming {
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }
  .naming input {
    width: 160px;
  }
  .retype {
    flex-wrap: nowrap;
  }
  .retype input {
    flex: 1;
    min-width: 0;
  }
  .x {
    padding: 2px 8px;
    color: var(--muted);
  }
  button {
    padding: 3px 9px;
  }
  .said {
    color: var(--muted);
    font-size: 12px;
  }
</style>
