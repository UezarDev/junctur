<script lang="ts">
  // The caption translation of the open recording (5k), under the Captions panel's "Caption text":
  // how many sentences are translated, a run's progress with Cancel, and Translate for the ones
  // left (a new cut can leave a few). Picking a language in the panel starts a run at once; the
  // model works in the background (src/translation.rs), so the editor is never held up, and the
  // preview shows what is translated so far. An export or a send waits for all of it.
  import * as api from "./api";
  import InfoTip from "./InfoTip.svelte";

  let {
    captions,
    source,
    cuts,
    onError,
  }: {
    captions: api.Captions;
    source: string | null;
    /** What the sentences depend on besides the language: the kept ranges, who says what, and the words fixed (5h). */
    cuts: string;
    onError: (message: string) => void;
  } = $props();

  let run = $state<api.TranslationRun | null>(null);
  let left = $state<{ missing: number; sentences: number } | null>(null);

  const to = $derived(captions.translate);
  const mine = $derived(!!run && run.file === source && run.to === to);
  const running = $derived(mine && !!run?.running);

  async function refresh() {
    try {
      const s = await api.translationStatus();
      run = s.run;
      left = { missing: s.missing, sentences: s.sentences };
    } catch {
      left = null;
    }
  }

  async function translate() {
    try {
      run = await api.translateCaptions();
    } catch (e) {
      onError(String(e));
    }
  }

  // A run's progress, and the count again when it ends.
  $effect(() => {
    const off = api.onTranslation((r) => {
      run = r;
      if (!r.running) refresh();
    });
    return () => {
      off.then((f) => f());
    };
  });

  // The count follows the language and the cuts, not every caption setting: counting builds every
  // sentence of the transcript. A language picked here, not the one the recording opened with,
  // starts translating.
  let seen: { source: string | null; lang: string } | null = null;
  $effect(() => {
    const lang = to;
    void cuts;
    refresh();
    if (seen && seen.source === source && lang && lang !== seen.lang) translate();
    seen = { source, lang };
  });
</script>

{#if to}
  <div class="translation">
    {#if running && run}
      <div class="bar" title="Translating on this machine">
        <div style="width: {run.total ? (100 * run.done) / run.total : 0}%"></div>
      </div>
      <span class="note">
        {#if run.done === 0}
          Starting… (the first run downloads the model, about 630 MB)
        {:else}
          Translating… {run.done.toLocaleString()} of {run.total.toLocaleString()} sentences
        {/if}
      </span>
      <button onclick={() => api.cancelTranslation().catch((e) => onError(String(e)))}>Cancel</button>
    {:else if left && left.missing > 0}
      <span class="note">
        {left.missing.toLocaleString()} of {left.sentences.toLocaleString()} sentences not translated yet
        <InfoTip
          label="sentences not translated"
          text="Until they are, the preview shows them as spoken and an export waits. The first translation downloads the model (about 630 MB)."
        />
      </span>
      <button onclick={translate}>Translate</button>
    {:else if left}
      <span class="note">All {left.sentences.toLocaleString()} sentences translated.</span>
    {/if}
    {#if mine && run && !run.running && (run.unknown > 0 || run.failed > 0)}
      <p class="error">
        {#if run.unknown > 0}{run.unknown.toLocaleString()} sentences had no transcript language to translate from.{/if}
        {#if run.failed > 0}The model gave no usable translation of {run.failed.toLocaleString()} sentences.{/if}
        Translate tries them again.
      </p>
    {/if}
    {#if mine && run?.error}<p class="error">{run.error}</p>{/if}
  </div>
{/if}

<style>
  .translation {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 8px;
    align-items: center;
    margin: 6px 0;
  }
  .bar {
    flex: 1 1 100%;
    height: 4px;
    background: var(--line);
    border-radius: 2px;
    overflow: hidden;
  }
  .bar div {
    height: 100%;
    background: var(--accent);
  }
  .note {
    flex: 1;
    color: var(--muted);
    font-size: 12px;
    line-height: 1.5;
  }
  .error {
    flex: 1 1 100%;
    color: var(--bad);
    font-size: 12px;
    margin: 0;
  }
</style>
