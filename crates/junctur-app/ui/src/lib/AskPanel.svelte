<script lang="ts">
  // The Ask tab (7c, a prototype): "ask, scrub, accept". No cuts are made up front: select a part on
  // the lanes (or nothing, for the whole recording), type what to do with it, and the request is read
  // as a recipe from a fixed menu (by a small text model, or by keywords without it), shown in plain
  // words with its numbers to change. What it would change waits as ghosts on the Ask lane; Preview
  // plays the edit as it would be, and nothing changes until you accept. Each request is a step in the
  // history, so the next one stacks on what you accepted.
  import * as api from "./api";
  import { clock } from "./time";

  let {
    view,
    editable,
    spans,
    previewing,
    asking,
    focus,
    onAsk,
    onRerun,
    onAnswer,
    onPreview,
    onPlay,
  }: {
    view: api.AskView | null;
    /** The list can be edited now (not while the recording's run is making it). */
    editable: boolean;
    /** The selection, a span per stretch (source seconds). */
    spans: { start: number; end: number }[];
    /** The ghosts play in place of the list. */
    previewing: boolean;
    asking: boolean;
    /** The ghost last clicked on the lane: step id and its index. */
    focus: { step: number; index: number } | null;
    onAsk: (request: string, range: [number, number] | null, model: api.AskSize | null) => Promise<void>;
    onRerun: (id: number, recipe: api.Recipe) => Promise<void>;
    onAnswer: (answer: api.AskAnswer, id: number, which?: number[], applied?: number) => Promise<void>;
    onPreview: (on: boolean) => Promise<void>;
    onPlay: (at: number) => void;
  } = $props();

  // Uezar's and the research's requests, to start from.
  const EXAMPLES = [
    "remove parts with silence and no movement",
    "keep only 1/4 second each time I break an ore, wood, or hit an enemy",
    "cut everything Tsunheto says about food",
    "cut when the inventory is open",
  ];
  const ROWS = 200;
  const MODEL_KEY = "junctur.askModel";

  let text = $state("");
  let reader = $state<api.AskSize | "off">(readModel());
  let busy = $state(false);
  function readModel(): api.AskSize | "off" {
    try {
      const v = localStorage.getItem(MODEL_KEY);
      return v === "large" || v === "off" ? v : "small";
    } catch {
      return "small";
    }
  }
  function setModel(v: api.AskSize | "off") {
    reader = v;
    try {
      localStorage.setItem(MODEL_KEY, v);
    } catch {
      // Remembered for this page only.
    }
  }
  const chosen = $derived(view?.models.find((m) => m.size === reader) ?? null);
  const gb = (m: api.AskModel) => (m.megabytes / 1000).toFixed(1);
  const selection = $derived.by((): [number, number] | null => {
    if (!spans.length) return null;
    const a = Math.min(...spans.map((s) => s.start));
    const b = Math.max(...spans.map((s) => s.end));
    return b - a > 0.05 ? [Math.round(a * 1000) / 1000, Math.round(b * 1000) / 1000] : null;
  });
  const steps = $derived(view ? [...view.steps].reverse() : []);
  const waiting = $derived(!!view && view.steps.some((s) => s.ghosts.length > 0));

  async function act(f: () => Promise<void>) {
    if (busy) return;
    busy = true;
    try {
      await f();
    } finally {
      busy = false;
    }
  }
  function ask() {
    const r = text.trim();
    if (!r) return;
    void act(async () => {
      await onAsk(r, selection, reader === "off" ? null : reader);
    });
  }
  function readBy(s: api.AskStep): string {
    const r = s.reading;
    if (r.reader === "keywords") return `read by the keywords${r.fallback ? ` (${r.fallback})` : ""}`;
    if (r.reader === "edited") return "your recipe";
    const t = r.answerS != null ? ` in ${r.answerS} s` : "";
    const load = r.loadS != null ? `, model opened in ${r.loadS} s` : "";
    return `read by ${r.reader}${r.device ? ` on the ${r.device === "cuda" ? "GPU" : "CPU"}` : ""}${t}${load}`;
  }
  const seconds = (g: api.AskGhost[], keep: boolean) =>
    Math.round(g.filter((x) => x.keep === keep).reduce((n, x) => n + x.end - x.start, 0) * 10) / 10;
  function scope(s: api.AskStep): string {
    return s.range ? `${clock(s.range[0])} to ${clock(s.range[1])}` : "the whole recording";
  }
  // The numbers a recipe's plain words name, as the user edits them before asking again.
  let edits = $state<Record<number, Partial<api.Recipe>>>({});
  let json = $state<Record<number, string>>({});
  function numbers(s: api.AskStep): ("min_seconds" | "before" | "after")[] {
    const a = s.recipe?.action;
    if (a === "cut_where" || a === "keep_where") return ["min_seconds"];
    if (a === "keep_around" || a === "cut_around" || a === "clip_each") return ["before", "after"];
    return [];
  }
  const LABEL = { min_seconds: "Longer than", before: "Before", after: "After" };
  // What the engine uses when the recipe gives none (junctur-core recipe::MIN_PART_S, AROUND, CLIP).
  function value(s: api.AskStep, k: "min_seconds" | "before" | "after"): number {
    const e = edits[s.id]?.[k];
    if (typeof e === "number") return e;
    const r = s.recipe?.[k];
    if (typeof r === "number") return r;
    if (k === "min_seconds") return 1;
    if (s.recipe?.action === "clip_each") return 2;
    return k === "before" ? 0.1 : 0.15;
  }
  function again(s: api.AskStep) {
    if (!s.recipe) return;
    const r: api.Recipe = { ...s.recipe, ...edits[s.id] };
    void act(() => onRerun(s.id, r));
  }
  function againJson(s: api.AskStep) {
    let r: api.Recipe;
    try {
      r = JSON.parse(json[s.id] ?? "");
    } catch (e) {
      json[s.id] = `${json[s.id] ?? ""}`;
      alert(`That is not JSON: ${e}`);
      return;
    }
    void act(() => onRerun(s.id, r));
  }
  let rows: Record<string, HTMLElement | undefined> = {};
  $effect(() => {
    if (focus) rows[`${focus.step}:${focus.index}`]?.scrollIntoView({ block: "nearest" });
  });
</script>

<div class="ask">
  <h3>Ask</h3>
  <p class="hint">
    Nothing is cut up front. Select a part on the lanes (or nothing, for the whole recording), say what to do with it,
    look at the ghosts on the Ask lane, preview, and accept.
  </p>
  {#if !view}
    <p class="hint">Reading the requests…</p>
  {:else}
    <section>
      <p class="scope">Over: {selection ? `${clock(selection[0])} to ${clock(selection[1])} (the selection)` : "the whole recording"}</p>
      <textarea
        rows="2"
        placeholder="remove parts with silence and no movement"
        bind:value={text}
        onkeydown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            ask();
          }
        }}
      ></textarea>
      <div class="row wrap">
        <button class="primary" disabled={!text.trim() || busy || asking} onclick={ask}>{asking ? "Reading…" : "Ask"}</button>
        <label class="field" title="A small text model reads the request as a recipe; without one, a keyword reader does"
          >Read with
          <select value={reader} onchange={(e) => setModel((e.currentTarget as HTMLSelectElement).value as api.AskSize | "off")}>
            {#each view.models as m (m.size)}
              <option value={m.size}
                >{m.size === "small" ? "Qwen3 1.7B" : "Qwen3 4B, better"}{m.downloaded ? "" : ` (downloads ${gb(m)} GB the first time)`}</option
              >
            {/each}
            <option value="off">keywords only</option>
          </select>
        </label>
      </div>
      {#if asking && chosen && !chosen.loaded}
        <p class="hint">{chosen.downloaded ? "Opening the model…" : `Downloading the model the first time (${gb(chosen)} GB)…`}</p>
      {/if}
      <div class="examples">
        {#each EXAMPLES as e (e)}
          <button class="example" onclick={() => (text = e)}>{e}</button>
        {/each}
      </div>
      {#if !view.examples}
        <p class="hint">Game sounds ("each time I break an ore") are found like the one you mark in the Like this tab: mark one there first.</p>
      {/if}
    </section>
    {#if waiting}
      <section class="row">
        <button class:on={previewing} disabled={busy} onclick={() => act(() => onPreview(!previewing))}
          >{previewing ? "Stop the preview" : "Preview the ghosts"}</button
        >
        <span class="hint">{previewing ? "Playing the edit as it would be, skipping the ghost cuts." : "Play the edit as if every ghost were accepted."}</span>
      </section>
    {/if}
    {#each steps as s (s.id)}
      <section class="step">
        <p class="request">“{s.request}” <span class="hint">over {scope(s)}</span></p>
        <p class="hint">{readBy(s)}</p>
        {#if s.described}<p class="recipe">{s.described}</p>{/if}
        {#if s.error}<p class="error">{s.error}</p>{/if}
        {#each s.notes ?? [] as n (n)}<p class="hint">{n}</p>{/each}
        {#if s.recipe && numbers(s).length}
          <div class="row pads">
            {#each numbers(s) as k (k)}
              <label class="field"
                >{LABEL[k]}
                <input
                  type="number"
                  min="0"
                  step="0.05"
                  value={value(s, k)}
                  onchange={(e) => (edits[s.id] = { ...edits[s.id], [k]: Math.max(0, Number((e.currentTarget as HTMLInputElement).value) || 0) })}
                />
              </label>
            {/each}
            <span class="hint">s</span>
            <button disabled={busy || !edits[s.id]} onclick={() => again(s)}>Ask again with these</button>
          </div>
        {/if}
        {#if s.recipe}
          <details>
            <summary>Recipe</summary>
            <textarea class="mono" rows="3" value={json[s.id] ?? JSON.stringify(s.recipe)} oninput={(e) => (json[s.id] = (e.currentTarget as HTMLTextAreaElement).value)}></textarea>
            <button disabled={busy || json[s.id] == null} onclick={() => againJson(s)}>Run this recipe</button>
          </details>
        {/if}
        {#if s.ghosts.length}
          <div class="row wrap">
            <span class="count">
              {s.ghosts.filter((g) => !g.keep).length} cuts ({seconds(s.ghosts, false)} s){s.ghosts.some((g) => g.keep)
                ? `, ${s.ghosts.filter((g) => g.keep).length} keeps (${seconds(s.ghosts, true)} s)`
                : ""} waiting
            </span>
            <button class="primary" disabled={!editable || busy} onclick={() => act(() => onAnswer("accept", s.id))}>Accept all</button>
            <button disabled={busy} onclick={() => act(() => onAnswer("reject", s.id))}>Reject all</button>
          </div>
          <ul class="ghosts">
            {#each s.ghosts.slice(0, ROWS) as g, i (g.start)}
              <li class:keep={g.keep} class:lit={focus?.step === s.id && focus.index === i} bind:this={rows[`${s.id}:${i}`]}>
                <button class="when mono" onclick={() => onPlay(Math.max(0, g.start - 1.5))} title="Play from just before it">{clock(g.start)}</button>
                <span class="what">{g.keep ? "keep" : "cut"} · {Math.round((g.end - g.start) * 10) / 10} s</span>
                <button disabled={!editable || busy} onclick={() => act(() => onAnswer("accept", s.id, [i]))} title="Accept this one">✓</button>
                <button disabled={busy} onclick={() => act(() => onAnswer("reject", s.id, [i]))} title="Reject this one">✗</button>
              </li>
            {/each}
          </ul>
          {#if s.ghosts.length > ROWS}<p class="hint">and {s.ghosts.length - ROWS} more on the lane</p>{/if}
        {:else if !s.sent && ((s.marks?.length ?? 0) > 0 || (s.clips?.length ?? 0) > 0)}
          <div class="row wrap">
            <span class="count">{s.marks?.length ? `${s.marks.length} marks` : `${s.clips?.length} clips`} for the Moments tab</span>
            <button class="primary" disabled={busy} onclick={() => act(() => onAnswer("accept", s.id))}>Send to Moments</button>
            <button disabled={busy} onclick={() => act(() => onAnswer("reject", s.id))}>Reject</button>
          </div>
        {/if}
        {#if s.sent}
          <div class="done">
            <span>{s.marks?.length ? `${s.marks.length} marks` : `${s.clips?.length ?? 0} clips`} in Moments</span>
            <button disabled={busy} onclick={() => act(() => onAnswer("undo", s.id))}>Take back</button>
          </div>
        {/if}
        {#each s.applied as a (a.id)}
          <div class="done">
            <span>Accepted {a.ghosts.length} ({clock(Math.min(...a.ghosts.map((g) => g.start)))} to {clock(Math.max(...a.ghosts.map((g) => g.end)))})</span>
            <button disabled={!editable || busy} onclick={() => act(() => onAnswer("undo", s.id, undefined, a.id))}>Undo</button>
          </div>
        {/each}
        {#if s.rejected}<p class="hint">{s.rejected} rejected</p>{/if}
      </section>
    {/each}
  {/if}
</div>

<style>
  .ask {
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: 6px;
    border-top: 1px solid var(--line);
  }
  section.row {
    flex-direction: row;
    align-items: center;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .wrap {
    flex-wrap: wrap;
  }
  .hint,
  .count,
  .scope {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }
  .request,
  .recipe {
    margin: 0;
    font-size: 12px;
  }
  .recipe {
    padding: 4px 6px;
    border-radius: 6px;
    background: var(--raised);
  }
  .error {
    margin: 0;
    font-size: 12px;
    color: var(--bad);
  }
  textarea {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    font: inherit;
    font-size: 12px;
  }
  .examples {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .example {
    font-size: 11px;
    padding: 1px 6px;
  }
  .field {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .pads input {
    width: 56px;
  }
  details {
    font-size: 12px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 240px;
    overflow-y: auto;
  }
  li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 6px;
    border-radius: 6px;
    border-left: 3px solid var(--bad);
    background: var(--raised);
  }
  li.keep {
    border-left-color: var(--accent);
  }
  li.lit {
    outline: 1px solid var(--accent);
  }
  .when {
    padding: 1px 6px;
  }
  .what {
    flex: 1;
    font-size: 12px;
  }
  .done {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .done span {
    flex: 1;
  }
  .on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
