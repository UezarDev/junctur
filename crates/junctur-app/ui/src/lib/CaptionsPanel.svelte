<script lang="ts">
  // The caption styling panel (5a): captions on or off, each speaker's colour and name with a
  // sample of how it reads, how cues are cut, and exporting the SRT. Everything is saved to
  // edit.junctur at once (the `captions` and `speaker` lines), so it is readable and survives a
  // re-cut. Only what an SRT carries into DaVinci Resolve is offered: text, timing and colour.
  // Size, font and position are set in Resolve (an SRT import drops them).
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
  let exported = $state<string | null>(null);
  let busy = $state(false);

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
      const n = await api.exportCaptions(path);
      exported = `${n.toLocaleString()} captions written to ${path}`;
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
      {#if exported}<p class="done">{exported}</p>{/if}
      <p class="note">
        Cues follow the edited timeline, like the render and the FCPXML. In DaVinci Resolve: File &gt; Import &gt;
        Subtitle, then place it at the start of the timeline; the colours come through. An SRT carries no size
        or position: set those in Resolve. Premiere Pro is untested and likely shows its default style.
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
  .done {
    color: var(--good);
    font-size: 12px;
    word-break: break-all;
  }
</style>
