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
  // 5j, how cues are cut) are
  // `CaptionRules`, shared with Settings (2i); a list without
  // its own settings follows the user's defaults, and "Save as default" makes this recording's the
  // defaults.
  import { save } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import { captionFace, captionFont } from "./captionStyle";
  import CaptionRules from "./CaptionRules.svelte";

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
      ];
    } catch (e) {
      onError(String(e));
    } finally {
      sending = false;
    }
  }
  let installed = $state<string | null>(null);
  async function installScript() {
    installed = null;
    try {
      installed = await api.installResolveScript();
    } catch (e) {
      onError(String(e));
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
      <p class="note">Open a recording with an edit list to set up its captions.</p>
    {:else}
      <CaptionRules value={captions} part="switches" onChange={(c) => setCaptions(c)} {onError} />

      <h3>Speakers</h3>
      {#if speakers.length === 0}
        <p class="note">No speakers in this edit list yet; captions will be white. Run the pipeline to tell them apart.</p>
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

      <h3>Style</h3>
      <CaptionRules value={captions} part="style" onChange={(c) => setCaptions(c)} {onError} />
      <CaptionRules value={captions} part="look" onChange={(c) => setCaptions(c)} {onError} />
      <p class="note">
        The preview and "Send to Resolve" use all of these. An SRT brings only the colour, bold, italic and underline
        into Resolve: there the font, size, outline and position are the subtitle track's style (select the track,
        Inspector &gt; Track). Resolve has been reported to give bold or italic (perhaps underlined) SRT captions
        their own style rather than the track's: if a Track Style change then misses them, turn these off.
      </p>

      <h3>People talking at once</h3>
      <CaptionRules value={captions} part="overlap" {names} onChange={(c) => setCaptions(c)} {onError} />
      <p class="note">
        Where their captions go in the preview and in Resolve. The protagonist keeps the normal spot and the others
        move round them. Side by side needs short lines (about 20 characters per line) not to run into each other.
        The SRT always stacks them in one caption, first to speak on top.
      </p>

      <h3>Caption text</h3>
      <CaptionRules value={captions} part="text" onChange={(c) => setCaptions(c)} {onError} />
      <p class="note">
        What the captions show, in the SRT, in Resolve and in the preview; the transcript keeps the words as
        spoken, so clearing one brings them back. Find and replace runs first, then case, punctuation and censoring.
      </p>

      <h3>How captions are cut</h3>
      <CaptionRules value={captions} part="cut" onChange={(c) => setCaptions(c)} {onError} />
      <div class="defaults">
        <span class="note">
          {edit?.captionsOwn ? "Set for this recording." : "Following your defaults (Settings)."}
        </span>
        <button onclick={saveDefault} title="New recordings, and ones not set by hand, start from these">
          Save as default
        </button>
      </div>
      {#if savedDefault}<p class="done">Saved as the default for every recording.</p>{/if}

      <button class="export" onclick={exportSrt} disabled={busy || !source}>
        {busy ? "Writing…" : "Export captions (.srt)…"}
      </button>
      {#if exported}
        {#each exported as line (line)}<p class="done">{line}</p>{/each}
      {/if}
      <p class="note">
        Cues follow the edited timeline, like the render and the FCPXML. When people talk at once, one caption
        holds them all, each speaker on their own line in their colour; the line and word limits apply to each speaker.
        In DaVinci Resolve import the file (File &gt; Import &gt; Subtitle) onto one subtitle track at the start
        of the timeline; the colours, bold, italic and underline come through. An SRT carries no size or position: set
        those in Resolve. Premiere Pro is untested and likely shows its default style.
      </p>

      <h3>Send to Resolve</h3>
      <CaptionRules value={captions} part="resolve" onChange={(c) => setCaptions(c)} {onError} />
      <button class="export" onclick={sendResolve} disabled={sending || !source}>
        {sending ? "Sending…" : "Send to Resolve"}
      </button>
      {#if sentLines}
        {#each sentLines as line (line)}<p class="done">{line}</p>{/each}
      {/if}
      <button onclick={installScript} title="Puts the Junctur script into Resolve's Scripts folder; sending does this too">
        Install script in Resolve
      </button>
      {#if installed}<p class="done">Installed: {installed}</p>{/if}
      <p class="note">
        Places every caption on the open Resolve timeline as a Text+ clip, each speaker on their own video track
        ("Junctur" and their name) in their colour, styled as above, at the edited timeline's times: put the
        edited video (render or FCPXML) at the start of that timeline. Once per Resolve session, choose Workspace
        &gt; Scripts &gt; Junctur in Resolve and leave it running; Junctur installs the script when you send.
        Sending again updates the captions already there instead of adding more. The template is Junctur's
        own Text+ unless you name a Text+ title from Resolve's media pool; your template keeps its shadow and
        background, these settings set the rest.
      </p>

      <h3>Remembered voices</h3>
      {#if voiceList.length === 0}
        <p class="note">
          None yet. Select some words only one person says in the transcript and choose "Remember voice…": every
          recording then names that person's speaker the same, in the same colour. Any length works, even one
          word, and the words picked are that person's from then on; half a minute from a few places makes the
          voice recognisable elsewhere. Add more by remembering the same name again.
        </p>
      {/if}
      {#each voiceList as v (v.name)}
        <div class="speaker">
          <span class="swatch" style:background={v.color}></span>
          <span class="name">{v.name} <span class="note">({v.seconds.toFixed(0)} s)</span></span>
          <button onclick={() => forget(v.name)} title="Forget {v.name}'s voice in every recording">Forget</button>
        </div>
      {/each}
      <button class="export" onclick={rehear} disabled={rehearing || !source}>
        {rehearing ? "Analysing speakers…" : "Analyse speakers again"}
      </button>
      {#if reheard}<p class="done">{reheard}</p>{/if}
      <p class="note">
        Remembering a voice relabels at once and keeps the words you picked as that person's. Analysing again
        also reads the recording again, with its voice windows cut where your picks begin and end: it takes
        about as long as the first speaker analysis, and remembering or forgetting a voice is not possible
        until it is done.
      </p>
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
    padding: 4px 12px 16px;
  }
  h3 {
    margin: 14px 0 8px;
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .note {
    color: var(--muted);
    font-size: 12px;
    line-height: 1.5;
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
  .name {
    flex: 1;
    min-width: 0;
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
    margin-top: 8px;
  }
  .export {
    margin-top: 16px;
    width: 100%;
  }
  .swatch {
    width: 32px;
    height: 18px;
    border-radius: 3px;
    border: 1px solid var(--line);
  }
  .done {
    color: var(--good);
    font-size: 12px;
    word-break: break-all;
  }
</style>
