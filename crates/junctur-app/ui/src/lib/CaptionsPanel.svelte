<script lang="ts">
  // The caption styling panel (5a, 5d): exporting captions with a render, previewing them over the
  // picture (two switches since 5d), each speaker's colour and name with a sample of how it reads,
  // bold, italic and underline (5f), how cues are cut, and exporting the SRT. Everything is saved to edit.junctur
  // at once (the `captions` and `speaker` lines), so it is readable and survives a re-cut. An SRT
  // carries only text, timing, colour, bold, italic and underline into DaVinci Resolve (5d, 5f); the
  // font, size, position, outline and where people talking at once go (5g) reach it through "Send to
  // Resolve", Text+ clips placed by the Junctur script running in Resolve, and drive the preview.
  // Export writes one file
  // (5e): a Resolve subtitle track shows one caption at a time, so people talking at once share one
  // caption, a line each; 5b's per-speaker files left beside it are listed. The remembered voices
  // are listed with Forget, and the speakers can be analysed again from the audio (5c), the seconds
  // picked for each voice cutting the windows; that reads the recording again, so it is a button,
  // never automatic. The caption settings' own fields (export, preview, style, the text options of
  // 5j and the translation of 5k, whose run `CaptionTranslation` shows, how cues are cut) are
  // `CaptionRules`, shared with Settings (2i); a list without
  // its own settings follows the user's defaults, and "Save as default" makes this recording's the
  // defaults. Each group is a `Section`; what used to be a paragraph under it is its (i) (2t), and
  // only status and results show as text.
  import { save } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import { captionFace, captionFont } from "./captionStyle";
  import CaptionRules from "./CaptionRules.svelte";
  import CaptionTranslation from "./CaptionTranslation.svelte";
  import InfoTip from "./InfoTip.svelte";
  import Section from "./Section.svelte";
  import { section } from "./captionSections";

  let {
    edit,
    source,
    onError,
  }: {
    edit: api.EditState | null;
    source: string | null;
    onError: (message: string) => void;
  } = $props();

  const captions = $derived(edit?.captions ?? null);
  const speakers = $derived(edit?.speakers ?? []);
  const usable = $derived(!!edit?.editable && !!captions);
  const face = $derived(captionFace(captions ?? { bold: false, italic: false, underline: false }));
  const font = $derived(captions ? captionFont(captions) : "inherit");
  const names = $derived(speakers.map((s) => s.name));
  // A shared group's heading and (i) (captionSections.ts).
  const head = (part: Parameters<typeof section>[0]) => ({ title: section(part).title, info: section(part).info });
  let exported = $state<string[] | null>(null);
  let busy = $state(false);
  let savedDefault = $state(false);

  // The remembered voices, read again whenever the speakers change (an enrollment relabels them).
  let voiceList = $state<api.Voice[]>([]);
  $effect(() => {
    void JSON.stringify(edit?.speakers ?? []);
    api
      .voices()
      .then((v) => (voiceList = v))
      .catch(() => (voiceList = []));
  });

  let rehearing = $state(false);
  let reheard = $state<string | null>(null);
  async function rehear() {
    rehearing = true;
    reheard = null;
    try {
      await api.reanalyzeSpeakers();
      reheard = "Speakers analysed again.";
    } catch (e) {
      onError(String(e));
    } finally {
      rehearing = false;
    }
  }

  async function forget(name: string) {
    await call(() => api.forgetVoice(name));
    voiceList = await api.voices().catch(() => []);
  }

  async function call(f: () => Promise<unknown>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }

  function setCaptions(change: Partial<api.Captions>) {
    if (!captions) return;
    call(() => api.setCaptions({ ...captions, ...change }));
  }

  // Send to Resolve (5g): one at a time; the reply says what landed.
  let sending = $state(false);
  let sentLines = $state<string[] | null>(null);
  async function sendResolve() {
    sending = true;
    sentLines = null;
    try {
      const r = await api.sendToResolve();
      const n = (k: number, what: string) => `${k.toLocaleString()} ${what}`;
      sentLines = [
        `On "${r.timeline}": ${n(r.placed, "placed")}, ${n(r.restyled, "updated")}, ${n(r.removed, "removed")}.`,
        ...(r.failed > 0 ? [`${n(r.failed, "not placed")}: ${r.firstError ?? "no reason given"}`] : []),
        ...(r.failed === 0 && r.firstError ? [r.firstError] : []),
        ...(r.unstyled.length > 0
          ? [`The template has no ${r.unstyled.join(", ")}: those keep the template's own look.`]
          : []),
      ];
    } catch (e) {
      onError(String(e));
    } finally {
      sending = false;
    }
  }

  async function saveDefault() {
    if (!captions) return;
    savedDefault = false;
    try {
      await api.saveCaptionDefaults(captions);
      savedDefault = true;
    } catch (e) {
      onError(String(e));
    }
  }

  function rename(id: number, e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const name = el.value.trim();
    const was = speakers.find((s) => s.id === id)?.name ?? "";
    if (!name) {
      el.value = was;
      return;
    }
    if (name !== was) call(() => api.renameSpeaker(id, name));
  }

  function stem(path: string): string {
    const file = path.split(/[\\/]/).pop() ?? "captions";
    const dot = file.lastIndexOf(".");
    return dot > 0 ? file.slice(0, dot) : file;
  }

  async function exportSrt() {
    if (!source) return;
    exported = null;
    try {
      const dir = source.slice(0, Math.max(source.lastIndexOf("\\"), source.lastIndexOf("/")) + 1);
      const path = await save({
        defaultPath: `${dir}${stem(source)} (edited).srt`,
        filters: [{ name: "SubRip captions", extensions: ["srt"] }],
      });
      if (!path) return;
      busy = true;
      const r = await api.exportCaptions(path);
      exported = [
        `${r.cues.toLocaleString()} captions: ${path}`,
        ...r.stale.map((p) => `A speaker file from an earlier export: delete it before importing: ${p}`),
      ];
    } catch (e) {
      onError(String(e));
    } finally {
      busy = false;
    }
  }
</script>

<section class="panel">
  <header>
    <span>Captions</span>
  </header>

  <div class="body">
    {#if !usable || !captions}
      <p class="status">Open a recording with an edit list to set up its captions.</p>
    {:else}
      <Section {...head("switches")}>
        <CaptionRules value={captions} part="switches" onChange={(c) => setCaptions(c)} {onError} />
      </Section>

      <Section title="Speakers" info="Each speaker's caption colour and name, with a sample of how it reads.">
        {#if speakers.length === 0}
          <p class="status">No speakers yet: captions are white. Run the pipeline to tell them apart.</p>
        {/if}
        {#each speakers as s (s.id)}
          <div class="speaker">
            <input
              type="color"
              value={s.color}
              title="Caption colour for {s.name}"
              onchange={(e) => call(() => api.recolorSpeaker(s.id, (e.currentTarget as HTMLInputElement).value))}
            />
            <input class="name" value={s.name} maxlength="60" aria-label="Name of speaker {s.id}" onchange={(e) => rename(s.id, e)} />
          </div>
          <div
            class="sample"
            style:color={s.color}
            style:font-weight={face.weight}
            style:font-style={face.style}
            style:text-decoration={face.decoration}
            style:font-family={font}
          >
            {s.name}: ¿qué tal? Hola a todos
          </div>
        {/each}
      </Section>

      <Section {...head("style")}>
        <CaptionRules value={captions} part="style" onChange={(c) => setCaptions(c)} {onError} />
        <CaptionRules value={captions} part="look" onChange={(c) => setCaptions(c)} {onError} />
      </Section>

      <Section {...head("overlap")}>
        <CaptionRules value={captions} part="overlap" {names} onChange={(c) => setCaptions(c)} {onError} />
      </Section>

      <Section {...head("text")}>
        <CaptionRules value={captions} part="text" onChange={(c) => setCaptions(c)} {onError} />
        <CaptionTranslation
          {captions}
          {source}
          cuts={`${edit?.ranges.join(",") ?? ""}|${edit?.speakersKey ?? ""}|${edit?.fixesKey ?? ""}`}
          {onError}
        />
      </Section>

      <Section {...head("cut")}>
        <CaptionRules value={captions} part="cut" onChange={(c) => setCaptions(c)} {onError} />
      </Section>

      <div class="defaults">
        <span class="status">
          {edit?.captionsOwn ? "Set for this recording." : "Following your defaults (Settings)."}
        </span>
        <button onclick={saveDefault} title="New recordings, and ones not set by hand, start from these">
          Save as default
        </button>
      </div>
      {#if savedDefault}<p class="status good">Saved as the default for every recording.</p>{/if}

      <Section
        title="Export (.srt)"
        info="Cues follow the edited timeline, like the render and the FCPXML; people talking at once share one caption, a line each. In Resolve: File > Import > Subtitle, onto one subtitle track at the timeline's start. Size and position are set there. Premiere Pro is untested."
      >
        <button class="wide" onclick={exportSrt} disabled={busy || !source}>
          {busy ? "Writing…" : "Export captions (.srt)…"}
        </button>
        {#if exported}
          {#each exported as line (line)}<p class="status good path">{line}</p>{/each}
        {/if}
      </Section>

      <Section {...head("resolve")}>
        <CaptionRules value={captions} part="resolve" onChange={(c) => setCaptions(c)} {onError} />
        <div class="buttons">
          <button class="grow" onclick={sendResolve} disabled={sending || !source}>
            {sending ? "Sending…" : "Send to Resolve"}
          </button>
        </div>
        {#if sentLines}
          {#each sentLines as line (line)}<p class="status good path">{line}</p>{/each}
        {/if}
      </Section>

      <Section
        title="Remembered voices"
        info="Select words only one person says in the transcript and choose Remember voice…: every recording then names that speaker the same, in the same colour. Half a minute from a few places makes a voice recognisable; remember the same name again to add more."
      >
        {#if voiceList.length === 0}<p class="status">None yet.</p>{/if}
        {#each voiceList as v (v.name)}
          <div class="speaker">
            <span class="swatch" style:background={v.color}></span>
            <span class="name">{v.name} <span class="seconds">({v.seconds.toFixed(0)} s)</span></span>
            <button onclick={() => forget(v.name)} title="Forget {v.name}'s voice in every recording">Forget</button>
          </div>
        {/each}
        <div class="buttons">
          <button class="grow" onclick={rehear} disabled={rehearing || !source}>
            {rehearing ? "Analysing speakers…" : "Analyse speakers again"}
          </button>
          <InfoTip
            label="Analyse speakers again"
            text="Reads the recording again, its voice windows cut where your picks begin and end. Takes about as long as the first speaker analysis; voices cannot be remembered or forgotten until it is done."
          />
        </div>
        {#if reheard}<p class="status good">{reheard}</p>{/if}
      </Section>
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
    align-items: center;
    padding: 10px 12px 8px;
    font-weight: 600;
    border-bottom: 1px solid var(--line);
  }
  .body {
    flex: 1;
    overflow-y: auto;
    padding: 4px 12px 20px;
  }
  .speaker {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 8px;
  }
  .speaker input[type="color"] {
    width: 32px;
    height: 26px;
    padding: 0;
    border: 1px solid var(--line);
    background: none;
  }
  .speaker input.name {
    font: inherit;
    color: inherit;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 4px 8px;
  }
  .name {
    flex: 1;
    min-width: 0;
  }
  .seconds {
    color: var(--muted);
    font-size: 12px;
  }
  .sample {
    margin: 4px 0 2px 40px;
    padding: 6px 10px;
    border-radius: 4px;
    background: #000;
    font-weight: 600;
    text-shadow:
      0 0 2px #000,
      0 0 2px #000;
  }
  .defaults {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    margin-top: 10px;
  }
  .defaults .status {
    margin: 0;
  }
  .defaults button {
    flex: none;
    white-space: nowrap;
  }
  .wide {
    width: 100%;
    margin-top: 6px;
  }
  .buttons {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
  }
  .grow {
    flex: 1;
  }
  .swatch {
    width: 32px;
    height: 18px;
    border-radius: 3px;
    border: 1px solid var(--line);
  }
  .path {
    word-break: break-all;
  }
</style>
