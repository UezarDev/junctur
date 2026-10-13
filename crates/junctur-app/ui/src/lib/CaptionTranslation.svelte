<script lang="ts">
  // The caption translation of the open recording (5k), under the Captions panel's "Caption text":
  // how many sentences are translated, a run's progress with Cancel, and Translate for the ones
  // left (a new cut can leave a few). Picking a language in the panel starts a run at once; the
  // model works in the background (src/translation.rs), so the editor is never held up, and the
  // preview shows what is translated so far. An export or a send waits for all of it.
  import * as api from "./api";

  let {
    captions,
    source,
    onError,
  }: {
    captions: api.Captions;
    source: string | null;
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

  // The count follows the language and the cuts (a new edit state); a language picked here, not
  // the one the recording opened with, starts translating.
  let seen: { source: string | null; lang: string } | null = null;
  $effect(() => {
    const lang = to;
    void captions;
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
        Translating… {run.done.toLocaleString()} of {run.total.toLocaleString()} sentences
      </span>
      <button onclick={() => api.cancelTranslation().catch((e) => onError(String(e)))}>Cancel</button>
    {:else if left && left.missing > 0}
      <span class="note">
        {left.missing.toLocaleString()} of {left.sentences.toLocaleString()} sentences not translated yet; until
        they are, the preview shows them as spoken and the export waits.
      </span>
      <button onclick={translate} title="The first translation downloads the model (about 630 MB)">Translate</button>
    {:else if left}
      <span class="note">All {left.sentences.toLocaleString()} sentences translated.</span>
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
  }
  .error {
    flex: 1 1 100%;
    color: var(--bad);
    margin: 0;
  }
</style>
