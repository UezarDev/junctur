<script lang="ts">
  // The Correct pop-up: what to analyse again (the words, the speakers) and what of the user's to
  // keep (the cut, names and voices, corrected words), over the selection or the whole timeline.
  // A dialog of its own, not the word bar's: the choices are many and the run is long, so they are
  // read before anything starts. App.svelte opens it as it opens Settings, so the app's keys stand
  // down meanwhile, and runs what it asks (asking first before a fresh cut over the user's edits).
  import * as api from "./api";
  import Field from "./Field.svelte";
  import Section from "./Section.svelte";
  import { slot } from "./floating.svelte";
  import { stamp } from "./transcript";

  let {
    selection,
    onRun,
    onClose,
  }: {
    /** The selection in source seconds and what it is ("12 words", "4.20 s of timeline"); null when nothing is selected. */
    selection: { start: number; end: number; what: string } | null;
    /** Starts the run over `within` (null: the whole timeline); resolves to why it did not start, or
     * null once started or when the user chose not to (then the dialog stays). */
    onRun: (ask: Omit<api.Ask, "within" | "replace">, within: [number, number] | null) => Promise<{ error: string | null; started: boolean }>;
    onClose: () => void;
  } = $props();

  // Correct's defaults: everything heard again, everything of the user's kept.
  let words = $state(true);
  let speakers = $state(true);
  let keepCuts = $state(true);
  let keepNames = $state(true);
  let keepFixes = $state(true);
  let running = $state(false);
  let error = $state<string | null>(null);

  // Naming the speakers afresh hears them again.
  const hears = $derived(speakers || !keepNames);
  // Why Re-analyse selection cannot run as ticked, for its tooltip; null when it can. The speakers
  // are heard over the whole recording only (a stretch's would match no one outside it).
  const selectionWhy = $derived(
    !selection
      ? "Nothing is selected: select a stretch on the timeline, or words in the transcript, to analyse only that part"
      : hears
        ? "The speakers are heard over the whole recording only: untick “Hear the speakers again” and keep “Names and voices” to analyse the selection"
        : !words && keepCuts && keepFixes
          ? "Nothing to redo in the selection: tick what to redo, or untick what not to keep"
          : null,
  );
  const span = $derived(selection ? `${stamp(selection.start * 1000)} to ${stamp(selection.end * 1000)}` : null);

  async function run(whole: boolean) {
    if (running || (!whole && selectionWhy)) return;
    running = true;
    error = null;
    try {
      const within: [number, number] | null = whole || !selection ? null : [selection.start, selection.end];
      const r = await onRun({ words, speakers: hears, keepCuts, keepNames, keepFixes }, within);
      error = r.error;
      if (r.started) onClose();
    } finally {
      running = false;
    }
  }

  function focus(el: HTMLButtonElement) {
    el.focus();
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.code !== "Escape") return;
    e.stopPropagation();
    // An open (i) takes Escape first.
    if (slot.open) slot.open = null;
    else if (!running) onClose();
  }}
/>

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && !running && onClose()}>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="reanalyse-title">
    <h2 id="reanalyse-title">Correct this recording</h2>
    <p class="lead">Analyse it again, part by part: tick what to redo and what of yours to keep.</p>

    <Section title="Redo">
      <Field
        kind="check"
        label="Transcribe the words again"
        info="Whisper hears the speech again with your word list as it is now; for the whole timeline about as long as the first transcription took, the words coming back five minutes at a time. Unticked, the words stay as they are."
      >
        {#snippet children(id)}<input {id} type="checkbox" bind:checked={words} />{/snippet}
      </Field>
      <Field
        kind="check"
        label="Hear the speakers again"
        info={keepNames
          ? "Who talks when is worked out again from the audio, against the remembered voices; the seconds you pinned to people still decide theirs. Whole timeline only. Unticked, the speakers stay as they are."
          : "Naming the speakers afresh hears them again. Whole timeline only."}
      >
        {#snippet children(id)}<input
            {id}
            type="checkbox"
            checked={hears}
            disabled={!keepNames}
            onchange={(e) => (speakers = e.currentTarget.checked)}
          />{/snippet}
      </Field>
    </Section>

    <Section title="Keep">
      <Field
        kind="check"
        label="Cuts"
        info="Your cut stays as it is. Unticked, a fresh cut is made from the analysis (only inside the selection, for Re-analyse selection); when the list holds your edits you are asked first, and your version is kept as edit.junctur.bak (or .bak2, …)."
      >
        {#snippet children(id)}<input {id} type="checkbox" bind:checked={keepCuts} />{/snippet}
      </Field>
      <Field
        kind="check"
        label="Names and voices"
        info="The names you gave the speakers, and the seconds you pinned to people. Unticked, the speakers are heard again and named afresh, the remembered voices still naming who they recognise (they are never deleted); your list and the pins are kept in backups. Whole timeline only."
      >
        {#snippet children(id)}<input {id} type="checkbox" bind:checked={keepNames} />{/snippet}
      </Field>
      <Field
        kind="check"
        label="Corrected words"
        info="Your fixed words, and the word times you set by moving a cut's edge or a word's own edge. Kept, they move onto the new words, and no other word is given a fix; any whose word is heard otherwise now is listed above the transcript. Unticked, they are dropped (only inside the selection, for Re-analyse selection), your list kept as a backup."
      >
        {#snippet children(id)}<input {id} type="checkbox" bind:checked={keepFixes} />{/snippet}
      </Field>
    </Section>

    <p class="status where">
      {#if selection}Selection: {selection.what}, {span}.{:else}Nothing selected.{/if}
    </p>
    {#if error}<p class="status bad">{error}</p>{/if}
    <div class="actions">
      <button onclick={onClose} disabled={running}>Cancel</button>
      <span class="grow"></span>
      <!-- A disabled button shows no tooltip, so the reason sits on a wrapper. -->
      <span title={selectionWhy ?? `Only ${span}: the rest of the transcript and the cut stay as they are`}>
        <button disabled={running || selectionWhy != null} onclick={() => run(false)}>Re-analyse selection</button>
      </span>
      <button
        class="primary"
        use:focus
        disabled={running}
        title="The whole recording, as ticked (Enter)"
        onclick={() => run(true)}>{running ? "Starting…" : "Re-analyse whole timeline"}</button
      >
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: absolute;
    inset: 0;
    z-index: 40;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.55);
  }
  .dialog {
    width: min(480px, calc(100% - 32px));
    max-height: calc(100% - 32px);
    overflow-y: auto;
    padding: 18px 22px;
    background: var(--panel-solid);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.6);
    box-sizing: border-box;
    user-select: text;
  }
  h2 {
    margin: 0 0 4px;
    font-size: 16px;
  }
  .lead {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }
  .where {
    margin-top: 16px;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 12px;
  }
  .grow {
    flex: 1;
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
