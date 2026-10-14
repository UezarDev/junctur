<script lang="ts">
  // Settings (2i): a dialog over the editor or the start window, opened from the title bar's gear
  // (or File > Settings…, Ctrl+,). What it edits is junctur/config.json, the file `junctur analyze`
  // reads too (src/settings.rs): the Whisper model, the word list Whisper is told (5h) and the
  // caption defaults a recording without its own starts from. Saving never analyses anything; App.svelte asks again whether the open
  // recording is out of date, and 2d's notice in Properties offers the re-analysis.
  // Laid out as the Captions panel is (2t): the same sections, explanations behind (i)s.
  import { onMount } from "svelte";
  import * as api from "./api";
  import CaptionRules from "./CaptionRules.svelte";
  import Field from "./Field.svelte";
  import InfoTip from "./InfoTip.svelte";
  import Section from "./Section.svelte";

  let { onClose, onSaved }: { onClose: () => void; onSaved: (v: api.SettingsView) => void } = $props();

  let view = $state<api.SettingsView | null>(null);
  let draft = $state<api.Settings | null>(null);
  let error = $state<string | null>(null);
  let saving = $state(false);
  // The word list as the text box holds it, one entry per line; the core trims it and drops blank
  // lines and repeats when it saves.
  let wordsText = $state("");
  const model = $derived(view?.models.find((m) => m.key === draft?.whisperModel) ?? null);

  onMount(() => {
    api.settings().then(
      (v) => {
        view = v;
        draft = structuredClone(v.settings);
        wordsText = v.settings.words.join("\n");
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
      onSaved(await api.saveSettings({ ...$state.snapshot(draft), words: wordsText.split("\n") }));
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
    if (e.code === "Escape") {
      e.stopPropagation();
      onClose();
    }
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
          info={`${model ? `${model.note} ` : ""}Used by the next analysis; recordings already transcribed keep their transcript until re-analysed (Properties says when one was made with another model).`}
        >
          {#snippet children(id)}
            <select
              {id}
              value={draft?.whisperModel}
              onchange={(e) => draft && (draft.whisperModel = e.currentTarget.value)}
            >
              {#each view?.models ?? [] as m (m.key)}<option value={m.key}>{m.label}</option>{/each}
            </select>
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
      </Section>

      <h3 class="group">
        Caption defaults
        <InfoTip
          label="Caption defaults"
          text="What a recording starts from until its captions are set in its Captions tab."
        />
      </h3>
      <Section title="Export and preview">
        <CaptionRules value={draft.captions} part="switches" {onChange} onError={(e) => (error = e)} />
      </Section>
      <Section title="Style">
        <CaptionRules value={draft.captions} part="style" {onChange} onError={(e) => (error = e)} />
        <CaptionRules value={draft.captions} part="look" {onChange} onError={(e) => (error = e)} />
      </Section>
      <Section title="People talking at once">
        <CaptionRules value={draft.captions} part="overlap" {onChange} onError={(e) => (error = e)} />
      </Section>
      <Section title="Caption text">
        <CaptionRules value={draft.captions} part="text" {onChange} onError={(e) => (error = e)} />
      </Section>
      <Section title="How captions are cut">
        <CaptionRules value={draft.captions} part="cut" {onChange} onError={(e) => (error = e)} />
      </Section>
      <Section title="Send to Resolve">
        <CaptionRules value={draft.captions} part="resolve" {onChange} onError={(e) => (error = e)} />
      </Section>

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
    font-size: 14px;
    font-weight: 600;
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
