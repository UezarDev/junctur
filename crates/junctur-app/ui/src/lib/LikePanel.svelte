<script lang="ts">
  // The Like this tab (7b, a prototype): "show me one". Mark a sound (the selection, or half a
  // second at the playhead), Find all like this, then move the slider until only the right ones
  // show: ✓ keeps a match shown, ✗ makes it a "not this" and searches again. What to do with all of
  // them is one button: keep a little around each and cut the rest, cut each (both proposals to
  // accept, with undo), or send each to the Moments list as a marker or a clip.
  import * as api from "./api";
  import { confirmed, shown } from "./like";
  import { clock } from "./time";

  let {
    view,
    editable,
    spans,
    playhead,
    tracks,
    focus,
    searching,
    onSettings,
    onSearch,
    onAct,
    onAnswer,
    onPlay,
  }: {
    view: api.LikeView | null;
    /** The list can be edited now (not while the recording's run is making it). */
    editable: boolean;
    /** The selection, a span per stretch (source seconds). */
    spans: { start: number; end: number }[];
    /** Where the playhead is (source seconds). */
    playhead: () => number | null;
    /** The audio tracks to pick from, 0-based, with their names. */
    tracks: { index: number; name: string }[];
    /** The point of the match last clicked on the lane, shown lit. */
    focus: number | null;
    searching: boolean;
    onSettings: (s: api.LikeSettings) => void;
    onSearch: () => Promise<void>;
    onAct: (act: api.LikeAct) => Promise<void>;
    onAnswer: (answer: api.LikeAnswer, id?: number) => Promise<void>;
    onPlay: (span: { start: number; end: number }) => void;
  } = $props();

  // The engine's own lengths (junctur-core like::MAX_EXAMPLE_S and CLICK_S).
  const MAX_EXAMPLE = 3;
  const CLICK = 0.5;
  const ROWS = 300;

  const list = $derived(shown(view));
  const selection = $derived.by(() => {
    if (!spans.length) return null;
    return { start: Math.min(...spans.map((s) => s.start)), end: Math.max(...spans.map((s) => s.end)) };
  });
  let busy = $state(false);

  function settings(v: api.LikeView): api.LikeSettings {
    return {
      track: v.track,
      examples: v.examples,
      range: v.range,
      threshold: v.threshold,
      before: v.before,
      after: v.after,
      confirmed: v.confirmed,
    };
  }
  function change(patch: Partial<api.LikeSettings>) {
    if (view) onSettings({ ...settings(view), ...patch });
  }
  const r3 = (x: number) => Math.round(x * 1000) / 1000;
  function addSelection() {
    if (!view || !selection) return;
    const start = r3(selection.start);
    const end = r3(Math.min(selection.end, start + MAX_EXAMPLE));
    change({ examples: [...view.examples, { start, end, point: start, positive: true }] });
  }
  function addPlayhead() {
    const t = playhead();
    if (!view || t == null) return;
    const start = r3(Math.max(0, t - CLICK / 2));
    change({ examples: [...view.examples, { start, end: r3(start + CLICK), point: r3(t), positive: true }] });
  }
  function removeExample(i: number) {
    if (view) change({ examples: view.examples.filter((_, k) => k !== i) });
  }
  async function act(f: () => Promise<void>) {
    if (busy) return;
    busy = true;
    try {
      await f();
    } finally {
      busy = false;
    }
  }
  function tick(m: api.LikeMatch) {
    if (!view) return;
    const on = confirmed(view, m);
    change({ confirmed: on ? view.confirmed.filter((p) => Math.abs(p - m.point) >= 1e-3) : [...view.confirmed, m.point] });
  }
  // ✗: the match's own window becomes a "not this", and the search runs again with it.
  function reject(m: api.LikeMatch) {
    if (!view) return;
    change({ examples: [...view.examples, { start: m.start, end: m.end, point: m.point, positive: false }] });
    void act(onSearch);
  }
  const canSearch = $derived(!!view && view.examples.some((e) => e.positive) && !searching);
  const wide = $derived(!!selection && selection.end - selection.start > MAX_EXAMPLE);
  const pad = $derived(view ? r3(view.before + view.after) : 0);
  function describe(p: api.LikeProposal): string {
    const n = `${p.matches} match${p.matches === 1 ? "" : "es"}`;
    return p.kind === "keepAround"
      ? `Keep ${pad} s around ${n} and cut the rest of ${clock(p.range[0])} to ${clock(p.range[1])}`
      : `Cut ${pad} s around ${n}`;
  }
  let rows: Record<number, HTMLElement | undefined> = {};
  $effect(() => {
    if (focus != null) rows[focus]?.scrollIntoView({ block: "nearest" });
  });
</script>

<div class="like">
  <h3>Like this</h3>
  <p class="hint">
    Mark a sound you want more of: select it on the lanes (up to {MAX_EXAMPLE} s), or play to it and add {CLICK} s at
    the playhead. Then find every moment that sounds like it.
  </p>
  {#if !view}
    <p class="hint">Reading the search…</p>
  {:else}
    <section>
      <div class="row">
        <button disabled={!selection} onclick={addSelection} title="The selection's first {MAX_EXAMPLE} s">Add the selection</button>
        <button onclick={addPlayhead}>Add at the playhead</button>
      </div>
      {#if view.examples.length}
        <ul class="examples">
          {#each view.examples as e, i (i)}
            <li class:not={!e.positive}>
              <button class="when mono" onclick={() => onPlay(e)} title="Hear it">{clock(e.start)}</button>
              <span class="what">{e.positive ? "like this" : "not this"} · {Math.round((e.end - e.start) * 100) / 100} s</span>
              <button class="x" onclick={() => removeExample(i)} title="Remove this example">×</button>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="empty">No example yet.</p>
      {/if}
    </section>
    <section>
      <label class="field"
        >Track
        <select
          value={view.effectiveTrack}
          onchange={(e) => change({ track: Number((e.currentTarget as HTMLSelectElement).value) })}
        >
          {#each tracks as t (t.index)}
            <option value={t.index}>{t.index + 1} · {t.name}{t.index === view.gameTrack ? " (game)" : ""}</option>
          {/each}
          {#if !tracks.length}<option value={view.effectiveTrack}>{view.effectiveTrack + 1}</option>{/if}
        </select>
      </label>
      <label class="check" title={wide ? "" : `Select more than ${MAX_EXAMPLE} s on the lanes to search only there`}>
        <input
          type="checkbox"
          checked={!!view.range}
          disabled={!view.range && !wide}
          onchange={(e) =>
            change({ range: (e.currentTarget as HTMLInputElement).checked && selection ? [r3(selection.start), r3(selection.end)] : null })}
        />
        {view.range ? `Only in ${clock(view.range[0])} to ${clock(view.range[1])}` : "Only in the selection"}
      </label>
      <div class="row">
        <button class="primary" disabled={!canSearch || busy} onclick={() => act(onSearch)}>
          {searching ? "Searching…" : "Find all like this"}
        </button>
        {#if searching && !view.ready.includes(view.effectiveTrack)}
          <span class="hint">reading this track's sound the first time</span>
        {:else if view.searched}
          <span class="hint">searched in {view.searched.seconds} s{view.searched.built ? " (sound read first)" : ""}</span>
        {/if}
      </div>
    </section>
    {#if view.searched}
      <section>
        <label class="field"
          >How similar
          <input
            type="range"
            min="0.3"
            max="1"
            step="0.01"
            value={view.threshold}
            oninput={(e) => change({ threshold: Number((e.currentTarget as HTMLInputElement).value) })}
          />
          <span class="mono">{Math.round(view.threshold * 100)}%</span>
        </label>
        <p class="count">
          {list.length} shown of {view.matches.length} found{view.searched.capped ? " (the closest only: there were more)" : ""}
        </p>
        {#if list.length}
          <ul class="matches">
            {#each list.slice(0, ROWS) as m (m.start)}
              <li class:lit={focus != null && Math.abs(focus - m.point) < 1e-3} bind:this={rows[m.point]}>
                <button class="when mono" onclick={() => onPlay(m)} title="Hear it">{clock(m.point)}</button>
                <span class="score"><span class="bar" style:width="{Math.round(m.score * 100)}%"></span></span>
                <span class="mono pct">{Math.round(m.score * 100)}%</span>
                <button class:on={confirmed(view, m)} onclick={() => tick(m)} title="Yes, like this: keep it shown">✓</button>
                <button disabled={busy || searching} onclick={() => reject(m)} title="Not this: search again without it">✗</button>
              </li>
            {/each}
          </ul>
          {#if list.length > ROWS}<p class="hint">and {list.length - ROWS} more on the lane</p>{/if}
        {/if}
      </section>
      <section>
        <div class="row pads">
          <label class="field"
            >Before
            <input
              type="number"
              min="0"
              step="0.05"
              value={view.before}
              onchange={(e) => change({ before: Math.max(0, Number((e.currentTarget as HTMLInputElement).value) || 0) })}
            />
          </label>
          <label class="field"
            >After
            <input
              type="number"
              min="0"
              step="0.05"
              value={view.after}
              onchange={(e) => change({ after: Math.max(0, Number((e.currentTarget as HTMLInputElement).value) || 0) })}
            />
          </label>
          <span class="hint">s around each match</span>
        </div>
        <div class="row wrap">
          <button disabled={!list.length || busy} onclick={() => act(() => onAct("keepAround"))} title="Keep only these bits of the searched range"
            >Keep around each, cut the rest</button
          >
          <button disabled={!list.length || busy} onclick={() => act(() => onAct("cutEach"))}>Cut each</button>
          <button disabled={!list.length || busy} onclick={() => act(() => onAct("mark"))} title="Each a moment: the half minute before it"
            >Mark each</button
          >
          <button disabled={!list.length || busy} onclick={() => act(() => onAct("clip"))} title="Each a moment of its own: before to after"
            >Clip each</button
          >
        </div>
        {#if view.proposal}
          <div class="proposal">
            <span>{describe(view.proposal)}?</span>
            <button class="primary" disabled={!editable || busy} onclick={() => act(() => onAnswer("accept"))}>Accept</button>
            <button disabled={busy} onclick={() => act(() => onAnswer("discard"))}>Discard</button>
          </div>
        {/if}
        {#each view.applied as a (a.id)}
          <div class="done">
            <span>{a.kind === "keepAround" ? "Kept around" : "Cut"} {a.matches} matches</span>
            <button disabled={!editable || busy} onclick={() => act(() => onAnswer("undo", a.id))}>Undo</button>
          </div>
        {/each}
        {#each view.sent as s (s.id)}
          <div class="done">
            <span>{s.points.length} {s.kind === "mark" ? "marks" : "clips"} in Moments</span>
            <button disabled={busy} onclick={() => act(() => onAnswer("takeBack", s.id))}>Take back</button>
          </div>
        {/each}
      </section>
    {/if}
  {/if}
</div>

<style>
  .like {
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
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .wrap {
    flex-wrap: wrap;
  }
  .hint,
  .empty,
  .count {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }
  .field,
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .field input[type="range"] {
    flex: 1;
  }
  .pads input {
    width: 56px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .matches {
    max-height: 260px;
    overflow-y: auto;
  }
  li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 6px;
    border-radius: 6px;
    border-left: 3px solid var(--accent);
    background: var(--raised);
  }
  li.not {
    border-left-color: var(--bad);
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
  .score {
    flex: 1;
    height: 6px;
    border-radius: 3px;
    background: var(--line);
    overflow: hidden;
  }
  .bar {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .pct {
    font-size: 11px;
    width: 32px;
    text-align: right;
  }
  .on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .x {
    padding: 0 6px;
  }
  .proposal,
  .done {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .proposal span,
  .done span {
    flex: 1;
  }
  .proposal {
    padding: 6px;
    border-radius: 6px;
    border: 1px solid var(--accent);
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
