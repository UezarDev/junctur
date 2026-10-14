<script lang="ts">
  // Settings (2i): a dialog over the editor or the start window, opened from the title bar's gear
  // (or File > Settings…, Ctrl+,). What it edits is junctur/config.json, the file `junctur analyze`
  // reads too (src/settings.rs): the Whisper model, the word list Whisper is told (5h), which words
  // the transcript flags for checking (5o) and the
  // caption defaults a recording without its own starts from. Saving never analyses anything; App.svelte asks again whether the open
  // recording is out of date, and 2d's notice in Properties offers the re-analysis.
  import { onMount } from "svelte";
  import * as api from "./api";
  import CaptionRules from "./CaptionRules.svelte";

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

  async function save() {
    if (!draft) return;
    saving = true;
    error = null;
    try {
      // An emptied or fractional number field: whole percent, within 0 to 100.
      const flagBelow = Math.min(100, Math.max(0, Math.round(Number(draft.flagBelow) || 0)));
      onSaved(await api.saveSettings({ ...$state.snapshot(draft), words: wordsText.split("\n"), flagBelow }));
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
      <p class="bad">The settings file could not be read ({view.error}). Saving replaces it.</p>
    {/if}
    {#if draft && view}
      <h3>Transcription</h3>
      <label class="field">
        <span>Whisper model</span>
        <select bind:value={draft.whisperModel}>
          {#each view.models as m (m.key)}<option value={m.key}>{m.label}</option>{/each}
        </select>
      </label>
      {#if model}<p class="note">{model.note}</p>{/if}
      <p class="note">
        Used by the next analysis. Recordings already transcribed keep their transcript until you re-analyse
        them (Properties says when one was made with another model).
      </p>
      <label class="words">
        <span>Word list</span>
        <textarea rows="5" spellcheck="false" bind:value={wordsText} placeholder="Uezar&#10;Tsunheto"></textarea>
      </label>
      <p class="note">
        Names and terms Whisper should spell this way, one per line, told to it before every stretch it hears. Used by
        the next transcription; recordings already transcribed are not redone. A word you fix in the transcript (double-click
        it) is added at the top on its own. When the list is too long for Whisper the first entries win.
      </p>
      <label class="field">
        <span>Flag words Whisper was less sure of than</span>
        <span><input class="pct" type="number" min="0" max="100" step="1" bind:value={draft.flagBelow} /> %</span>
      </label>
      <p class="note">
        Underlined in the transcript to check: fix one by double-clicking it, or mark it right. 0 flags none. Only
        recordings transcribed since this came in have it.
      </p>

      <h3>Caption defaults</h3>
      <p class="note">What a recording starts from until its captions are set in its Captions tab.</p>
      <CaptionRules value={draft.captions} onChange={(c) => draft && (draft.captions = c)} onError={(e) => (error = e)} />

      {#if view.path}<p class="note path">Saved in {view.path}</p>{/if}
    {:else if !error}
      <p class="note">Reading the settings…</p>
    {/if}
    {#if error}<p class="bad">{error}</p>{/if}
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
    width: min(460px, calc(100% - 32px));
    max-height: calc(100% - 32px);
    overflow-y: auto;
    padding: 16px 20px;
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
  h3 {
    margin: 16px 0 6px;
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .field {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    margin: 6px 0;
  }
  .pct {
    width: 56px;
  }
  .words {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 10px 0 4px;
  }
  .words textarea {
    resize: vertical;
    font: inherit;
  }
  .note {
    margin: 4px 0;
    color: var(--muted);
    font-size: 12px;
    line-height: 1.5;
  }
  .path {
    margin-top: 14px;
    word-break: break-all;
  }
  .bad {
    color: var(--bad);
    font-size: 12px;
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
