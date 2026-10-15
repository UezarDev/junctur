<script lang="ts">
  // Properties' Smart keep: the kind of video the user is making, whose words say how much of the
  // busy silence to keep (phase 3d's rules); ready prompts; Judge and how far it is; how the prompt
  // was read and what smart keep suggests (Accept all, Review the close calls). Nothing is applied
  // until the user accepts it.
  import * as api from "./api";
  import { clock } from "./time";

  let {
    file,
    view,
    onChanged,
    onReviewUncertain,
    onError,
  }: {
    /** The open recording. */
    file: string;
    view: api.KeepView | null;
    /** Something changed the view: read it again. */
    onChanged: () => void;
    /** Review the uncertain suggestions one at a time. */
    onReviewUncertain: () => void;
    onError: (message: string) => void;
  } = $props();

  // The box shows the recording's prompt, else the last one used, until the user types in it.
  let text = $state("");
  let typed = $state(false);
  let shownFor = "";
  $effect(() => {
    const v = view;
    if (file !== shownFor) {
      shownFor = file;
      typed = false;
    }
    if (!typed) text = v?.prompt ?? v?.last ?? "";
  });
  const run = $derived(view?.run ?? null);
  const running = $derived(run?.running ?? false);
  const s = $derived(view?.summary ?? null);
  const waiting = $derived((s?.cut ?? 0) + (s?.keep ?? 0));
  const stale = $derived(!!view?.judgedPrompt && view.judgedPrompt.trim() !== text.trim());
  let busy = $state(false);

  async function act(f: () => Promise<unknown>) {
    if (busy) return;
    busy = true;
    try {
      await f();
      onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      busy = false;
    }
  }
  const judge = () => act(() => api.keepJudge(file, text.trim()));
  const acceptAll = () => act(() => api.keepAnswer("acceptAll"));
  function preset(p: string) {
    text = p;
    typed = true;
  }
</script>

<section class="keep">
  <h3>Smart keep</h3>
  <p class="hint">
    Say what kind of video you are making. Talk stays as the cut leaves it; a silent stretch with a moment
    busier than this recording usually is (game audio, fight labels) is suggested as a keep, whole: fewer
    for highlights, more for a whole stream, none for a podcast. Nothing changes until you accept.
  </p>
  <textarea
    rows="3"
    placeholder="League highlights with friends: keep fights and funny moments"
    bind:value={text}
    oninput={() => (typed = true)}
    onblur={() => {
      if (typed && text.trim() && text.trim() !== (view?.prompt ?? "")) {
        void api.keepPrompt(file, text.trim()).catch((e) => onError(String(e)));
      }
    }}
  ></textarea>
  <div class="presets">
    {#each view?.presets ?? [] as p (p.name)}
      <button class="chip" title={p.prompt} onclick={() => preset(p.prompt)}>{p.name}</button>
    {/each}
  </div>
  <div class="row">
    <button
      class="primary"
      disabled={busy || running || !text.trim() || !view?.transcribed}
      title={view?.transcribed ? "" : "Smart keep runs once the transcript is done"}
      onclick={judge}
    >
      {view?.judgedPrompt ? "Judge again" : "Judge"}
    </button>
    {#if running}
      <span class="line">{run?.line ?? "judging…"}</span>
    {:else if run?.error}
      <span class="line failed" title={run.error}>{run.error}</span>
    {:else if !view?.transcribed}
      <span class="line">after the transcript</span>
    {/if}
  </div>

  {#if view?.judgedPrompt && s}
    {#if stale}
      <p class="note">Judged with another prompt; Judge again to use this one.</p>
    {/if}
    <p>
      {#if view.reading}Read as {view.reading}.{/if}
      {#if waiting > 0}
        Smart keep suggests keeping <b>{s.keep}</b> busy silent stretches ({clock(s.keepS)}){#if s.cut > 0}
          and cutting <b>{s.cut}</b> ({clock(s.cutS)}){/if}{#if s.uncertain > 0}; {s.uncertain} close calls{/if}.
      {:else}
        Smart keep agrees with the cut as it is.
      {/if}
      {#if s.accepted > 0}{s.accepted} accepted (undo one from the Suggestions lane).{/if}
    </p>
    <div class="row">
      <button disabled={busy || waiting === 0} onclick={acceptAll}>Accept all</button>
      <button disabled={busy || s.uncertain === 0} onclick={onReviewUncertain}>Review close calls</button>
    </div>
  {/if}
  {#if view && !view.on}
    <p class="hint">Off in Settings: a recording is judged only when you give it a prompt here.</p>
  {/if}
</section>

<style>
  .keep textarea {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    font: inherit;
  }
  .presets,
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: 6px 0;
  }
  .chip {
    padding: 1px 8px;
    border-radius: 999px;
    font-size: 12px;
  }
  .line {
    font-size: 12px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    flex: 1;
  }
  .failed {
    color: var(--bad);
  }
  .hint,
  .note {
    font-size: 12px;
    color: var(--muted);
  }
  .note {
    color: var(--accent);
  }
</style>
