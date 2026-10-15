<script lang="ts">
  // The recording's fixes as a list (5ae), in the Transcript panel in place of the words while it is
  // open: the way out of fixes that went where they should not (Uezar's sentence on 421 words, put
  // there by a field that saved what nobody typed and by 5aa's spread to every word heard alike).
  // Grouped by what they say, the biggest group first, since a wrong fix comes as one text on many
  // places; a group opens to its places, each with its time, what was heard there and a jump. The
  // ticked places or groups the filter shows go back to what was heard in one save (kept as heard,
  // as Undo those does, so Correct's spread does not bring them back). Chosen over an undo of recent
  // fixes: fixes are no undo steps (5h), and the wrong ones here were made far apart, between good
  // ones.
  //
  // Fixes of the heard text (Undo those, a removal, Looks right) are not listed: their words read as
  // heard already. Read again whenever the list's fixes change.
  import { SvelteSet } from "svelte/reactivity";
  import * as api from "./api";
  import { stamp } from "./transcript";

  let {
    edit,
    onJump,
    onClose,
    onError,
  }: {
    edit: api.EditState | null;
    /** Goes to words `first..=last`: selects them and plays from there. */
    onJump: (first: number, last: number) => void;
    onClose: () => void;
    onError: (message: string) => void;
  } = $props();

  let all = $state<api.ListedFix[]>([]);
  let loading = $state(true);
  let filter = $state("");
  let said = $state<string | null>(null);
  let busy = $state(false);
  // By the fix itself, not its place in `all`, so a list read again (any change to the fixes) keeps
  // the ticks of fixes still in it; only ticks the filter shows are removed.
  const ticked = new SvelteSet<string>();
  const idOf = (f: api.Fix) =>
    [f.track, f.start, f.nth, f.through?.start ?? "", f.through?.nth ?? "", f.text].join("|");
  const opened = new SvelteSet<string>();

  $effect(() => {
    void edit?.fixesKey;
    let gone = false;
    api.fixesList().then(
      (l) => {
        if (gone) return;
        all = l;
        loading = false;
        const still = new Set(l.map((f) => idOf(f.fix)));
        for (const t of [...ticked]) if (!still.has(t)) ticked.delete(t);
      },
      (e) => {
        if (gone) return;
        loading = false;
        onError(String(e));
      },
    );
    return () => (gone = true);
  });

  // What two hearings have in common, for the heard summary: case and the punctuation around aside.
  const like = (t: string) => t.toLowerCase().replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu, "");

  interface Group {
    text: string;
    places: number[];
    heard: [string, number][];
  }
  const groups = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    const by = new Map<string, number[]>();
    all.forEach((f, k) => {
      if (f.asHeard) return;
      if (q && !f.fix.text.toLowerCase().includes(q) && !f.heard.toLowerCase().includes(q)) return;
      const list = by.get(f.fix.text);
      if (list) list.push(k);
      else by.set(f.fix.text, [k]);
    });
    const out: Group[] = [];
    for (const [text, places] of by) {
      const count = new Map<string, number>();
      for (const k of places) {
        const h = like(all[k].heard) || "(no word now)";
        count.set(h, (count.get(h) ?? 0) + 1);
      }
      out.push({ text, places, heard: [...count].sort((a, b) => b[1] - a[1]) });
    }
    return out.sort((a, b) => b.places.length - a.places.length || a.text.localeCompare(b.text));
  });
  const listed = $derived(groups.reduce((n, g) => n + g.places.length, 0));
  // The ticked places the filter shows: what Remove takes back.
  const chosen = $derived(groups.flatMap((g) => g.places).filter((k) => ticked.has(idOf(all[k].fix))));

  function heardLine(g: Group): string {
    const shown = g.heard.slice(0, 4).map(([h, n]) => (n > 1 ? `${h} (${n.toLocaleString()})` : h));
    return `heard as ${shown.join(", ")}${g.heard.length > 4 ? ", …" : ""}`;
  }

  function groupState(g: Group): "all" | "some" | "none" {
    const n = g.places.filter((k) => ticked.has(idOf(all[k].fix))).length;
    return n === 0 ? "none" : n === g.places.length ? "all" : "some";
  }
  function tickGroup(g: Group, on: boolean) {
    for (const k of g.places) tick(k, on);
  }
  function tick(k: number, on: boolean) {
    const id = idOf(all[k].fix);
    if (on) ticked.add(id);
    else ticked.delete(id);
  }
  function toggle(text: string) {
    if (opened.has(text)) opened.delete(text);
    else opened.add(text);
  }

  async function remove() {
    const fixes = chosen.map((k) => all[k].fix);
    if (!fixes.length || busy) return;
    busy = true;
    said = null;
    try {
      const n = await api.removeFixes(fixes);
      said =
        n === fixes.length
          ? `${n.toLocaleString()} fix${n === 1 ? "" : "es"} taken back: those words read as heard.`
          : `${n.toLocaleString()} of ${fixes.length.toLocaleString()} taken back; the others changed meanwhile.`;
      for (const f of fixes) ticked.delete(idOf(f));
    } catch (e) {
      onError(String(e));
    } finally {
      busy = false;
    }
  }

  function setIndeterminate(el: HTMLInputElement, on: boolean) {
    el.indeterminate = on;
    return {
      update(v: boolean) {
        el.indeterminate = v;
      },
    };
  }
</script>

<div class="fixes">
  <div class="bar">
    <input class="filter" placeholder="Filter by text or heard words" bind:value={filter} aria-label="Filter the fixes" />
    <button disabled={!chosen.length || busy} onclick={() => void remove()} title="Take the ticked fixes shown back to what was heard there"
      >{busy ? "Removing…" : `Remove ${chosen.length ? chosen.length.toLocaleString() : ""}`}</button
    >
    <button class="x" onclick={onClose} title="Back to the transcript" aria-label="Close the list of fixes">×</button>
  </div>
  <div class="said muted" role="status">
    {#if loading}
      Reading the fixes…
    {:else if said}
      {said}
    {:else}
      {listed.toLocaleString()} fixed place{listed === 1 ? "" : "s"} in {groups.length.toLocaleString()} text{groups.length === 1 ? "" : "s"}{filter.trim() ? " matching" : ""}
    {/if}
  </div>
  <div class="list">
    {#each groups as g (g.text)}
      {@const st = groupState(g)}
      <div class="group">
        <input
          type="checkbox"
          checked={st === "all"}
          use:setIndeterminate={st === "some"}
          onchange={(e) => tickGroup(g, e.currentTarget.checked)}
          aria-label="Tick every place of “{g.text}”"
        />
        <button class="open" onclick={() => toggle(g.text)} aria-expanded={opened.has(g.text)}>
          <span class="arrow">{opened.has(g.text) ? "▾" : "▸"}</span>
          <span class="text">{g.text}</span>
          <span class="count">×{g.places.length.toLocaleString()}</span>
          <span class="heard muted">{heardLine(g)}</span>
        </button>
      </div>
      {#if opened.has(g.text)}
        {#each g.places as k (k)}
          {@const f = all[k]}
          <div class="place">
            <input type="checkbox" checked={ticked.has(idOf(f.fix))} onchange={(e) => tick(k, e.currentTarget.checked)} aria-label="Tick this place" />
            {#if f.first != null && f.last != null}
              {@const a = f.first}
              {@const b = f.last}
              <button class="jump mono" onclick={() => onJump(a, b)} title="Go there in the transcript">{stamp(f.fix.start * 1000)}</button>
              <span class="muted">heard “{f.heard}”</span>
            {:else}
              <span class="mono muted">{stamp(f.fix.start * 1000)}</span>
              <span class="muted">on no word now</span>
            {/if}
          </div>
        {/each}
      {/if}
    {/each}
  </div>
</div>

<style>
  .fixes {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .bar {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 8px 12px 4px;
  }
  .filter {
    flex: 1;
    min-width: 0;
  }
  .bar button {
    padding: 3px 9px;
  }
  .x {
    color: var(--muted);
  }
  .said {
    padding: 2px 12px 6px;
    font-size: 12px;
    border-bottom: 1px solid var(--line);
  }
  .muted {
    color: var(--muted);
  }
  .list {
    flex: 1;
    overflow-y: auto;
    padding: 4px 0 8px;
  }
  .group,
  .place {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 12px;
    min-width: 0;
  }
  .place {
    padding-left: 40px;
    font-size: 12px;
  }
  .open {
    display: flex;
    flex: 1;
    min-width: 0;
    gap: 6px;
    align-items: baseline;
    padding: 2px 4px;
    text-align: left;
    background: none;
    border: none;
  }
  .open:hover {
    background: rgba(255, 255, 255, 0.06);
  }
  .arrow {
    color: var(--muted);
    font-size: 11px;
  }
  .text {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 55%;
  }
  .count {
    color: var(--accent);
    font-weight: 600;
    font-size: 12px;
  }
  .heard {
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .jump {
    padding: 0 6px;
    font-size: 12px;
  }
  .mono {
    font-family: "Cascadia Mono", Consolas, monospace;
  }
</style>
