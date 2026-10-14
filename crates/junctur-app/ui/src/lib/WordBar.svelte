<script lang="ts">
  // What a selection of transcript words can do, at the bottom of the Transcript tab (2p; before, a
  // name field in the floating bar over the lanes). What is selected ("12 words in 3 places"), Cut
  // and Keep (every stretch in one undoable edit), and one button per person who already has a
  // name: the recording's named speakers and the voices Junctur remembers, in their colours, so
  // giving the words a speaker is one click. A click remembers that person's voice from every
  // selected stretch as one enrolment (what 5b's Remember voice… did with a typed name): the
  // seconds are theirs in this recording (5c pins) and they are recognised in others. "New name…"
  // is for someone not listed yet. The engine's answer shows on the line above.
  //
  // Fix text (5x) opens a field across the bar holding what the selected words show now, to retype
  // them as one text: saved at once as one fix for all of them (the word fixes inside go), its new
  // words added to the word list, an empty field or the heard words again taking the fixes away. In
  // the bar rather than in the transcript row: a sentence needs the room, and a row is one fixed
  // height in a virtual list. One stretch only: one text typed for several places has nowhere to go.
  //
  // A component of its own rather than more of TranscriptPanel: the panel is the virtual word list
  // and its pointer handling; this is the actions on what it selected, with its own fetch of the
  // remembered voices and its own busy state while an enrolment runs.
  import * as api from "./api";
  import * as sel from "./selection";
  import { shownText, type Words } from "./transcript";

  let {
    edit,
    words,
    selection,
    onSelect,
    onError,
    retyping = $bindable(false),
    onLearned,
  }: {
    edit: api.EditState | null;
    words: Words;
    /** A selection of words (the panel shows this bar only then). */
    selection: sel.Selection;
    onSelect: (s: sel.Selection | null) => void;
    onError: (message: string) => void;
    /** The Fix text field is open (the panel opens it on a double-click, 5x). */
    retyping?: boolean;
    /** The entries a fix added to the word list. */
    onLearned: (entries: string[]) => void;
  } = $props();

  // A speaker the analysis named "Speaker 3" has no name set yet: remembering a voice under that
  // label would make a person called "Speaker 3" in every recording, so those are not offered.
  const UNNAMED = /^Speaker \d+$/;

  let voices = $state<api.Voice[]>([]);
  let busy = $state(false);
  let said = $state<string | null>(null);
  let naming = $state(false);
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

  // Fix text (5x). The field is for the stretch it was opened on: another selection closes it.
  let draft = $state("");
  let shown = "";
  let openFor: string | null = null;
  let saving = $state(false);
  const one = $derived(runs.length === 1 ? { first: runs[0][0], last: runs[0][1] } : null);
  $effect(() => {
    const at = one ? `${one.first}-${one.last}` : null;
    if (!retyping) {
      openFor = null;
    } else if (!at) {
      retyping = false;
    } else if (openFor == null) {
      openFor = at;
      shown = shownText(words, one!);
      draft = shown;
    } else if (openFor !== at) {
      retyping = false;
    }
  });

  async function saveText() {
    if (!one || saving) return;
    if (draft.trim() === shown.trim()) {
      retyping = false;
      return;
    }
    saving = true;
    try {
      onLearned(await api.fixWords(one.first, one.last, draft));
      retyping = false;
    } catch (e) {
      onError(String(e));
    } finally {
      saving = false;
    }
  }

  // Selected once the bound value is in: selecting at mount, before it, left the caret at the end.
  function focusAll(el: HTMLInputElement) {
    el.focus();
    requestAnimationFrame(() => el.select());
  }

  // Another selection: the last answer was about the words before (an enrolment already on its
  // way still ends with its own line). An open "New name…" field and what is typed in it stay
  // (2p repair): Ctrl+clicking more of someone's lines while naming them is the point. They go
  // when the selection clears, which unmounts this bar.
  $effect(() => {
    void selection;
    said = null;
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
  {#if said}
    <div class="said">{said}</div>
  {/if}
  <div class="row">
    <span class="what mono">{sel.describe(selection, edit?.fps ?? null)}</span>
    <button onclick={() => call(() => sel.apply(selection, false))} title="Cut every selected stretch (Delete)">Cut</button>
    <button onclick={() => call(() => sel.apply(selection, true))} title="Keep every selected stretch (K)">Keep</button>
    <button
      disabled={!one}
      onclick={() => (retyping = !retyping)}
      title={one
        ? "Retype these words as one text: shown in the transcript and the captions in their place (double-click a selection does this too)"
        : "Select one stretch of words to fix its text: one text cannot go in several places"}>Fix text</button
    >
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
          if (e.key === "Escape") retyping = false;
        }}
        aria-label="What the selected words say"
      />
      <button disabled={saving} onclick={() => void saveText()}>Save</button>
      <button class="x" onclick={() => (retyping = false)} title="Leave the text as it was (Esc)" aria-label="Cancel">×</button>
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

<style>
  .wordbar {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 12px 10px;
    border-top: 1px solid var(--line);
    background: var(--panel-solid);
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
