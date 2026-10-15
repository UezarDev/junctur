<script lang="ts">
  // Settings: edits junctur/config.json, the file `junctur analyze` reads too. Saving never
  // analyses anything; App.svelte then asks whether the open recording is out of date.
  import { onMount } from "svelte";
  import * as api from "./api";
  import CaptionRules from "./CaptionRules.svelte";
  import Field from "./Field.svelte";
  import InfoTip from "./InfoTip.svelte";
  import ModelPicker, { chosenModel } from "./ModelPicker.svelte";
  import Section from "./Section.svelte";
  import { CAPTION_SECTIONS } from "./captionSections";
  import { slot } from "./floating.svelte";

  let { onClose, onSaved }: { onClose: () => void; onSaved: (v: api.SettingsView) => void } = $props();

  let view = $state<api.SettingsView | null>(null);
  let draft = $state<api.Settings | null>(null);
  let error = $state<string | null>(null);
  let saving = $state(false);
  // The word list as the text box holds it, one entry per line; the core trims it and drops blank
  // lines and repeats when it saves.
  let wordsText = $state("");
  // The labels added per kind, as their text boxes hold them, one per line; trimmed when saved.
  let addedText = $state<Record<api.LabelKindKey, string>>({ generic: "", league: "" });
  const kind = $derived(view?.labelKinds.find((k) => k.key === draft?.labels.kind));
  // Resolved as the picker resolves it, so the (i) explains the model the picker shows.
  const model = $derived(view ? chosenModel(view.models, draft?.whisperModel ?? "") : undefined);

  onMount(() => {
    api.settings().then(
      (v) => {
        view = v;
        draft = structuredClone(v.settings);
        wordsText = v.settings.words.join("\n");
        addedText = {
          generic: (v.settings.labels.added?.generic ?? []).join("\n"),
          league: (v.settings.labels.added?.league ?? []).join("\n"),
        };
      },
      (e) => (error = String(e)),
    );
    void api.uiLog("settings open");
  });

  function onChange(c: api.Captions) {
    if (draft) draft.captions = c;
  }

  async function save() {
    if (!draft) return;
    saving = true;
    error = null;
    try {
      // An emptied or fractional number field: whole percent, within 0 to 100.
      const flagBelow = Math.min(100, Math.max(0, Math.round(Number(draft.flagBelow) || 0)));
      const labels = {
        ...$state.snapshot(draft).labels,
        added: { generic: addedText.generic.split("\n"), league: addedText.league.split("\n") },
      };
      onSaved(await api.saveSettings({ ...$state.snapshot(draft), words: wordsText.split("\n"), flagBelow, labels }));
      onClose();
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.code !== "Escape") return;
    e.stopPropagation();
    // An open (i) takes Escape first: closing the dialog would lose the draft.
    if (slot.open) slot.open = null;
    else onClose();
  }}
/>

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && onClose()}>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="settings-title">
    <h2 id="settings-title">Settings</h2>
    {#if view?.error}
      <p class="status bad">The settings file could not be read ({view.error}). Saving replaces it.</p>
    {/if}
    {#if draft && view}
      <Section title="Transcription">
        <Field
          label="Whisper model"
          info={`${model ? `${model.name} is Whisper ${model.model}. ${model.note} ` : ""}Used by the next analysis; recordings already transcribed keep their transcript until re-analysed (Properties says when one was made with another model).`}
        >
          {#snippet children(id)}
            <ModelPicker
              {id}
              models={view?.models ?? []}
              value={draft?.whisperModel ?? ""}
              onChange={(v) => draft && (draft.whisperModel = v)}
            />
          {/snippet}
        </Field>
        <Field
          kind="stack"
          label="Word list"
          info="Names and terms Whisper should spell this way, one per line. Used by the next transcription. A word you fix in the transcript is added at the top; when the list is too long, the first entries win."
        >
          {#snippet children(id)}
            <textarea {id} rows="5" spellcheck="false" bind:value={wordsText} placeholder="Uezar&#10;Tsunheto"></textarea>
          {/snippet}
        </Field>
        <Field
          label="Flag words below (%)"
          info="Words Whisper was less sure of than this are underlined in the transcript to check: fix one by double-clicking it, or mark it right. 0 flags none. Only recordings transcribed since this came in have it."
        >
          {#snippet children(id)}
            <input
              {id}
              type="number"
              min="0"
              max="100"
              step="1"
              bind:value={() => draft?.flagBelow ?? 0, (v) => draft && (draft.flagBelow = v)}
            />
          {/snippet}
        </Field>
      </Section>

      <Section
        title="Labels"
        info="Labels the parts where nobody talks from the picture (SigLIP 2), with how busy the game sounds, on a lane of their own. Used by the next analysis; off by default, and the first labelled recording fetches about 1.5 GB of model."
      >
        <Field label="Label silent parts">
          {#snippet children(id)}
            <input
              {id}
              type="checkbox"
              bind:checked={() => draft?.labels.on ?? false, (v) => draft && (draft.labels.on = v)}
            />
          {/snippet}
        </Field>
        <Field label="Kind of video" info="Picks the built-in labels. League of Legends has the general ones and its own.">
          {#snippet children(id)}
            <select {id} bind:value={() => draft?.labels.kind ?? "generic", (v) => draft && (draft.labels.kind = v)}>
              {#each view?.labelKinds ?? [] as k (k.key)}
                <option value={k.key}>{k.name}</option>
              {/each}
            </select>
          {/snippet}
        </Field>
        {#if kind}
          <p class="status builtin">Built in: {kind.builtin.join(", ")}</p>
          <Field
            kind="stack"
            label="Your labels"
            info="More labels for this kind, one per line, scored by their own words: describe what the picture shows, like a video game boss fight."
          >
            {#snippet children(id)}
              <textarea {id} rows="3" spellcheck="false" bind:value={addedText[kind.key]} placeholder="a video game boss fight"
              ></textarea>
            {/snippet}
          </Field>
        {/if}
      </Section>

      <Section
        title="Smart keep"
        info="Talk stays as the cut leaves it; a silent stretch with a moment busier than the recording usually is (its game audio, or the fight labels) is suggested as a keep, whole, more or fewer as the kind of video you describe asks (Properties, Smart keep). Nothing is applied until you accept, and nothing is downloaded."
      >
        <Field
          label="Judge every analysed recording"
          info="With the last prompt you used, at the end of each analysis. Off, only a recording you gave a prompt is judged."
        >
          {#snippet children(id)}
            <input
              {id}
              type="checkbox"
              bind:checked={() => draft?.keep?.on ?? false, (v) => draft && (draft.keep = { ...draft.keep, on: v })}
            />
          {/snippet}
        </Field>
        {#if draft?.keep?.last}
          <p class="status builtin">Last prompt: {draft.keep.last}</p>
        {/if}
      </Section>

      <div class="group">
        <h3>Caption defaults</h3>
        <InfoTip
          label="Caption defaults"
          text="What a recording starts from until its captions are set in its Captions tab."
        />
      </div>
      {#each CAPTION_SECTIONS as s (s.part)}
        <Section title={s.title} info={s.info}>
          <CaptionRules value={draft.captions} part={s.part} {onChange} onError={(e) => (error = e)} />
          {#if s.part === "style"}
            <CaptionRules value={draft.captions} part="look" {onChange} onError={(e) => (error = e)} />
          {/if}
        </Section>
      {/each}

      {#if view.path}<p class="status path">Saved in {view.path}</p>{/if}
    {:else if !error}
      <p class="status">Reading the settings…</p>
    {/if}
    {#if error}<p class="status bad">{error}</p>{/if}
    <div class="actions">
      <button onclick={onClose}>Cancel</button>
      <button class="primary" onclick={save} disabled={!draft || saving}>{saving ? "Saving…" : "Save"}</button>
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
    width: min(520px, calc(100% - 32px));
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
  /* "Caption defaults" holds several sections, so it reads one step above their headings. */
  .group {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 28px 0 0;
  }
  .group h3 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }
  /* The dialog has the room for a wider control column than the panel's. */
  .dialog {
    --control-width: 240px;
  }
  .builtin {
    margin: 4px 0 8px;
    color: var(--muted);
  }
  .path {
    margin-top: 18px;
    word-break: break-all;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
