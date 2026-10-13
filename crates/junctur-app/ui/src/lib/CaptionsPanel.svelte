<script lang="ts">
  // The caption styling panel (5a): captions on or off, each speaker's colour and name with a
  // sample of how it reads, how cues are cut, and exporting the SRT. Everything is saved to
  // edit.junctur at once (the `captions` and `speaker` lines), so it is readable and survives a
  // re-cut. Only what an SRT carries into DaVinci Resolve is offered: text, timing and colour.
  // Size, font and position are set in Resolve (an SRT import drops them). Export writes one file per
  // speaker beside the combined one (5b), since a Resolve subtitle track shows one caption at a time;
  // the remembered voices are listed with Forget.
  import { save } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";

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
  let exported = $state<string[] | null>(null);
  let busy = $state(false);

  // The remembered voices, read again whenever the speakers change (an enrollment relabels them).
  let voiceList = $state<api.Voice[]>([]);
  $effect(() => {
    void JSON.stringify(edit?.speakers ?? []);
    api
      .voices()
      .then((v) => (voiceList = v))
      .catch(() => (voiceList = []));
  });

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

  // A number field commits on change, inside its limits; anything else snaps back on the next
  // edit state (the input shows the saved value again).
  function number(e: Event, key: "chars" | "lines" | "seconds", min: number, max: number) {
    const el = e.currentTarget as HTMLInputElement;
    const v = Number(el.value);
    if (!Number.isFinite(v) || v < min || v > max) {
      el.value = String(captions?.[key] ?? "");
      onError(`${key === "chars" ? "Characters per line" : key === "lines" ? "Lines" : "Seconds"} must be ${min} to ${max}`);
      return;
    }
    setCaptions({ [key]: key === "seconds" ? v : Math.round(v) });
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
        `${r.cues.toLocaleString()} captions, everyone: ${path}`,
        ...r.speakers.map((s) => `${s.cues.toLocaleString()} captions, ${s.name}: ${s.path}`),
        ...r.stale.map((p) => `Not written now, from an earlier export? Delete before importing: ${p}`),
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
    {#if captions}
      <label class="toggle" title="On: rendering also writes the captions beside the video">
        <input
          type="checkbox"
          checked={captions.on}
          disabled={!usable}
          onchange={(e) => setCaptions({ on: (e.currentTarget as HTMLInputElement).checked })}
        />
        {captions.on ? "On" : "Off"}
      </label>
    {/if}
  </header>

  <div class="body">
    {#if !usable || !captions}
      <p class="note">Open a recording with an edit list to set up its captions.</p>
    {:else}
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
        <div class="sample" style:color={s.color}>{s.name}: ¿qué tal? Hola a todos</div>
      {/each}

      <h3>How captions are cut</h3>
      <label class="field">
        <span>Characters per line</span>
        <input type="number" min="10" max="80" value={captions.chars} onchange={(e) => number(e, "chars", 10, 80)} />
      </label>
      <label class="field">
        <span>Lines per caption</span>
        <input type="number" min="1" max="3" value={captions.lines} onchange={(e) => number(e, "lines", 1, 3)} />
      </label>
      <label class="field">
        <span>Longest caption (s)</span>
        <input
          type="number"
          min="1"
          max="15"
          step="0.5"
          value={captions.seconds}
          onchange={(e) => number(e, "seconds", 1, 15)}
        />
      </label>

      <button class="export" onclick={exportSrt} disabled={busy || !source}>
        {busy ? "Writing…" : "Export captions (.srt)…"}
      </button>
      {#if exported}
        {#each exported as line (line)}<p class="done">{line}</p>{/each}
      {/if}
      <p class="note">
        Cues follow the edited timeline, like the render and the FCPXML. Each speaker also gets a file of their
        own, "(name).srt", with everything they say, also while someone else talks: in DaVinci Resolve import
        each one (File &gt; Import &gt; Subtitle) onto its own subtitle track at the start of the timeline, and
        overlapping talk shows at once; the colours come through. An SRT carries no size or position: set those
        in Resolve. Premiere Pro is untested and likely shows its default style.
      </p>

      <h3>Remembered voices</h3>
      {#if voiceList.length === 0}
        <p class="note">
          None yet. Select some words only one person says in the transcript and choose "Remember voice…": every
          recording then names that person's speaker the same, in the same colour. Half a minute from a few
          places works best; add more by remembering the same name again.
        </p>
      {/if}
      {#each voiceList as v (v.name)}
        <div class="speaker">
          <span class="swatch" style:background={v.color}></span>
          <span class="name">{v.name} <span class="note">({v.seconds.toFixed(0)} s)</span></span>
          <button onclick={() => forget(v.name)} title="Forget {v.name}'s voice in every recording">Forget</button>
        </div>
      {/each}
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
  .toggle {
    display: flex;
    gap: 6px;
    align-items: center;
    font-weight: 400;
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
  .field {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin: 6px 0;
  }
  .field input {
    width: 72px;
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
