<script lang="ts">
  // The caption panel: export and preview switches, each speaker's colour and name, the caption
  // settings (`CaptionRules`, shared with Settings), the SRT export, Send to Resolve and the voices.
  import { onDestroy, tick } from "svelte";
  import { save } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import { captionFace, captionFont } from "./captionStyle";
  import CaptionRules from "./CaptionRules.svelte";
  import CaptionTranslation from "./CaptionTranslation.svelte";
  import InfoTip from "./InfoTip.svelte";
  import Section from "./Section.svelte";
  import TimelineExists from "./TimelineExists.svelte";
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

  // Send to Resolve, one at a time. When the timeline exists, nothing changes until the pop-up is
  // answered, and a panel closed under it answers Cancel so the send never waits on a gone dialog.
  // Clips in the captions' way (only on Update captions only: a new timeline puts them on a new
  // track) hold the send (`inTheWay`) until the user picks; the re-send repeats the pop-up's answer
  // so it updates that timeline rather than making another.
  let sending = $state(false);
  let sendButton = $state<HTMLButtonElement | undefined>();
  let sentLines = $state<string[] | null>(null);
  let inTheWay = $state<{ clips: number; captions: number; answer: api.TimelineAnswer | null } | null>(null);
  // New track first: the one choice that never loses anything.
  let choice = $state<api.ExistingClips>("newTrack");
  let remember = $state(false);
  const CHOICES: { value: api.ExistingClips; label: string; note: string }[] = [
    {
      value: "newTrack",
      label: "Put them on a new track",
      note: "That speaker's new captions go on a track beside theirs; your clips stay as they are.",
    },
    {
      value: "replace",
      label: "Replace those clips",
      note: "Deletes only the clips a caption overlaps, then places every caption.",
    },
    {
      value: "skip",
      label: "Skip those captions",
      note: "Your clips stay; the captions they overlap are not placed.",
    },
  ];
  const CHOSEN: Record<api.ExistingClips, string> = {
    ask: "ask",
    newTrack: "put Junctur's captions on a new track",
    replace: "replace them",
    skip: "skip the captions they overlap",
  };
  // A question about one recording's send is not answered for the next one opened.
  $effect(() => {
    void source;
    inTheWay = null;
  });
  // The choice made in the question, sent; remembered for this recording, when asked to, only
  // once the send it was chosen for has gone through (a failed send leaves the recording asking).
  async function answer() {
    if (sending) return;
    const picked = choice;
    const keep = remember;
    if (!(await sendResolve(picked, inTheWay?.answer ?? null)) || !keep) return;
    const now = captions;
    if (now) await call(() => api.setCaptions({ ...now, existingClips: picked }));
  }
  // Resolves to whether the send went through (not refused, not stopped to ask). `sending` is set
  // before the first await, so a second click while one is on its way does nothing.
  let asking = $state<{ name: string; answer: (a: api.TimelineAnswer | null) => void } | null>(null);
  const ask = (name: string) =>
    new Promise<api.TimelineAnswer | null>((answer) => {
      asking = {
        name,
        answer: (a) => {
          asking = null;
          answer(a);
        },
      };
    });
  onDestroy(() => asking?.answer(null));
  // `existing` and `given` (the pop-up's answer) are a re-send's: the question about clips in the
  // way answered for the send that asked.
  async function sendResolve(
    existing?: api.ExistingClips,
    given: api.TimelineAnswer | null = null,
  ): Promise<boolean> {
    if (sending) return false;
    sending = true;
    sentLines = null;
    inTheWay = null;
    let asked = false;
    try {
      let used = given;
      let r = await api.sendToResolve(used, existing);
      if (r.kind === "exists") {
        asked = true;
        const answer = await ask(r.name);
        if (!answer) {
          sentLines = ["Nothing was sent."];
          return false;
        }
        used = answer;
        r = await api.sendToResolve(answer, existing);
      }
      // An answered send never asks again; if it did, say so rather than end without a word.
      if (r.kind !== "sent") {
        onError(`Resolve still has a timeline called "${r.name}" and nothing was sent: send again.`);
        return false;
      }
      if (r.inTheWay > 0) {
        choice = "newTrack";
        remember = false;
        inTheWay = { clips: r.inTheWay, captions: r.blocked, answer: used };
        // An update's note: a cut that no longer matches the edit's.
        sentLines = r.note ? [r.note] : null;
        return false;
      }
      const n = (k: number, what: string) => `${k.toLocaleString()} ${what}`;
      // The highlight and animation are not inputs a template lacks but a kind of template it is not.
      const MOTION = ["word highlight", "animation"];
      const motion = r.unstyled.filter((u) => MOTION.includes(u));
      const inputs = r.unstyled.filter((u) => !MOTION.includes(u));
      const captioned = r.placed + r.restyled + r.failed + r.skipped > 0;
      sentLines = [
        r.updated
          ? `Captions updated on timeline "${r.timeline}" in Resolve, now the open timeline; its cuts were left as they are.`
          : `Timeline "${r.timeline}" made in Resolve${r.replaced ? ", replacing the old one" : ""}.`,
        ...(r.note ? [r.note] : []),
        ...(captioned
          ? [`Captions: ${n(r.placed, "placed")}, ${n(r.restyled, "updated")}, ${n(r.removed, "removed")}.`]
          : []),
        ...(r.replacedClips > 0
          ? [`${n(r.replacedClips, r.replacedClips === 1 ? "clip of yours replaced" : "clips of yours replaced")}.`]
          : []),
        ...(r.skipped > 0 ? [`${n(r.skipped, "skipped")}, where your clips are.`] : []),
        ...(r.newTracks > 0
          ? [`${n(r.newTracks, r.newTracks === 1 ? "speaker's" : "speakers'")} new captions on a track beside theirs ("… (2)").`]
          : []),
        ...(r.failed > 0 ? [`${n(r.failed, "not placed")}: ${r.firstError ?? "no reason given"}`] : []),
        ...(r.failed === 0 && r.firstError ? [r.firstError] : []),
        ...(inputs.length > 0
          ? [`The template has no ${inputs.join(", ")}: those keep the template's own look.`]
          : []),
        ...(motion.length > 0
          ? [`No ${motion.join(" or ")}: your template is a plain Text+; only Junctur's own animated caption has them.`]
          : []),
      ];
      return true;
    } catch (e) {
      onError(String(e));
      return false;
    } finally {
      sending = false;
      if (asked) {
        await tick();
        sendButton?.focus();
      }
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

{#if asking}<TimelineExists name={asking.name} onPick={asking.answer} />{/if}

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

      <Section {...head("motion")}>
        <CaptionRules value={captions} part="motion" onChange={(c) => setCaptions(c)} {onError} />
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
          <button class="grow" bind:this={sendButton} onclick={() => sendResolve()} disabled={sending || !source}>
            {sending ? "Sending…" : "Send to Resolve"}
          </button>
        </div>
        {#if inTheWay}
          <div class="ask" role="group" aria-label="Clips already on Junctur's tracks">
            <p>
              {inTheWay.clips.toLocaleString()}
              {inTheWay.clips === 1 ? "clip" : "clips"} Junctur did not place
              {inTheWay.clips === 1 ? "is" : "are"} where {inTheWay.captions.toLocaleString()}
              {inTheWay.captions === 1 ? "caption goes" : "captions go"}, on Junctur's tracks. No caption was
              placed.
            </p>
            {#each CHOICES as c (c.value)}
              <span class="choice">
                <label>
                  <input type="radio" name="existing-clips" value={c.value} bind:group={choice} />
                  {c.label}
                </label>
                <InfoTip label={c.label} text={c.note} />
              </span>
            {/each}
            <label class="choice">
              <input type="checkbox" bind:checked={remember} />
              <span>Remember for this recording</span>
            </label>
            <div class="defaults">
              <button onclick={() => (inTheWay = null)} disabled={sending}>Cancel</button>
              <button onclick={answer} disabled={sending || !source}>Send</button>
            </div>
          </div>
        {/if}
        {#if captions.existingClips !== "ask"}
          <div class="defaults">
            <span class="status">
              Clips in the way: {CHOSEN[captions.existingClips]} (remembered for this recording).
            </span>
            <button onclick={() => setCaptions({ existingClips: "ask" })}>Ask again</button>
          </div>
        {/if}
        {#if sentLines}
          {#each sentLines as line (line)}<p class="status good path">{line}</p>{/each}
        {/if}
      </Section>

      <Section
        title="Remembered voices"
        info="Select words only one person says in the transcript and pick their name (or New name…) in the bar below it: they are relabelled at once, the words picked stay theirs, and every recording then names that speaker the same, in the same colour. Half a minute from a few places makes a voice recognisable; remember the same name again to add more."
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
  /* The question when clips Junctur did not place are in its captions' way. */
  .ask {
    margin-top: 8px;
    padding: 8px 10px;
    border: 1px solid var(--line);
    border-radius: 4px;
    font-size: 12px;
    line-height: 1.5;
  }
  .ask p {
    margin: 0 0 6px;
  }
  .choice {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 4px;
  }
  .choice label {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
</style>
