<script lang="ts">
  // What the app shows with nothing open (2d; laid out as 2g's mock S), in the spirit of Resolve's
  // project chooser: the recordings opened before on the left, each with a still, its length and
  // folder and where its analysis stands, and a place to drop or pick a recording on the right.
  // Dropping works anywhere on the window (App.svelte listens); this only draws the target. A new
  // recording opens straight into the editor and is analysed there; nothing here waits on analysis.
  //
  // The list is drawn from recent.json at once; each row then asks for its details and still in
  // the background (`recent_info`, `thumbnail`), so twenty recordings never hold the list up; at
  // most WORKERS rows at a time, top first, as each may start an ffprobe and an ffmpeg.
  import { onMount } from "svelte";
  import * as api from "./api";
  import { clock } from "./time";
  import * as steps from "./steps";
  import TrackSteps from "./TrackSteps.svelte";

  let {
    ready,
    dragging,
    onPick,
    onOpen,
    onError,
  }: {
    /** The player started (libmpv found): opening is possible. */
    ready: boolean;
    dragging: boolean;
    onPick: () => void;
    onOpen: (path: string) => void;
    onError: (e: string) => void;
  } = $props();

  interface Row {
    info?: api.RecordingInfo;
    progress?: api.Progress | null;
    still?: string;
  }

  let recent = $state<api.Recent[]>([]);
  let rows = $state<Record<string, Row>>({});
  const urls: string[] = [];

  function patch(path: string, r: Partial<Row>) {
    rows = { ...rows, [path]: { ...rows[path], ...r } };
  }

  const WORKERS = 2;
  let gone = false;

  /** Fills the rows' details, `WORKERS` at a time, in list order. */
  async function fill(list: api.Recent[]) {
    let next = 0;
    const worker = async () => {
      while (!gone && next < list.length) await details(list[next++]);
    };
    await Promise.all(Array.from({ length: WORKERS }, worker));
  }

  async function details(r: api.Recent) {
    if (r.missing) return;
    api.analysis(r.path).then((p) => patch(r.path, { progress: p }), () => {});
    try {
      patch(r.path, { info: await api.recentInfo(r.path) });
    } catch {
      // The row keeps "…": the project cannot be read.
    }
    try {
      const bytes = await api.thumbnail(r.path);
      const url = URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" }));
      urls.push(url);
      patch(r.path, { still: url });
    } catch {
      // No still (no video stream, ffmpeg missing): the row shows without one.
    }
  }

  const sameFile = (a: string, b: string) => a.replace(/\//g, "\\").toLowerCase() === b.replace(/\//g, "\\").toLowerCase();

  onMount(() => {
    api.recent().then(
      (r) => {
        recent = r;
        void fill(r);
      },
      (e) => onError(String(e)),
    );
    const off = api.onAnalysis((p) => {
      const hit = recent.find((r) => sameFile(r.path, p.file));
      if (hit) patch(hit.path, { progress: p });
    });
    return () => {
      gone = true;
      off.then((f) => f());
      urls.forEach((u) => URL.revokeObjectURL(u));
    };
  });

  async function forget(path: string) {
    try {
      await api.forgetRecent(path);
      recent = recent.filter((r) => r.path !== path);
    } catch (e) {
      onError(String(e));
    }
  }

  /** The badge: where the analysis stands. */
  function badge(r: api.Recent, row: Row | undefined): { text: string; cls: string } {
    if (r.missing) return { text: "Not found", cls: "bad" };
    const busy = steps.label(row?.progress ?? null);
    if (busy) return { text: busy, cls: "busy" };
    const i = row?.info;
    if (!i) return { text: "…", cls: "muted" };
    if (i.ranges === 0) return { text: "Ready · nothing kept", cls: "muted" };
    if (i.ranges != null) return { text: `Ready · ${i.ranges} ${i.ranges === 1 ? "range" : "ranges"}`, cls: "good" };
    if (i.stagesDone > 0) return { text: "Partly analysed", cls: "muted" };
    return { text: "Not analysed", cls: "muted" };
  }
</script>

<div class="start">
  <section class="left">
    <h1>Junctur</h1>
    <p class="tag">Cut long recordings down before Resolve.</p>
    <h2>Recent</h2>
    {#if recent.length === 0}
      <p class="none">Recordings you open show here.</p>
    {:else}
      <ul>
        {#each recent as r (r.path)}
          {@const row = rows[r.path]}
          {@const b = badge(r, row)}
          <li class:missing={r.missing}>
            <button
              class="open"
              disabled={!ready || r.missing}
              title={r.missing ? `${r.path} (not found)` : r.path}
              onclick={() => onOpen(r.path)}
            >
              <span class="still">
                {#if row?.still}<img src={row.still} alt="" />{/if}
              </span>
              <span class="what">
                <span class="name">{r.name}</span>
                <span class="meta mono">{row?.info?.duration ? clock(row.info.duration) : "--:--"} · {r.folder}</span>
                {#if steps.current(row?.progress ?? null).length}
                  <span class="bars"><TrackSteps steps={steps.current(row?.progress ?? null)} compact /></span>
                {/if}
              </span>
              <span class="badge {b.cls}"><span class="dot"></span>{b.text}</span>
            </button>
            <button class="x" title="Remove from the list" aria-label="Remove {r.name} from the list" onclick={() => forget(r.path)}
              >×</button
            >
          </li>
        {/each}
      </ul>
    {/if}
  </section>
  <section class="right">
    <button class="drop" class:dragging onclick={onPick} disabled={!ready}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3v12m0 0-5-5m5 5 5-5M4 20h16" /></svg>
      <span class="big">Drop a recording here</span>
      <span class="small">MKV, MP4, MOV… multi-track OBS files work best</span>
      <span class="cta">Open recording…</span>
    </button>
    <p class="note">The editor opens at once; analysis runs in the background and fills in as it goes.</p>
  </section>
</div>

<style>
  .start {
    position: absolute;
    inset: 0;
    display: flex;
    gap: 40px;
    padding: 36px 48px 28px;
    box-sizing: border-box;
    background: var(--panel-solid);
  }
  .left {
    flex: 1.15;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  h1 {
    margin: 0;
    font-size: 26px;
    font-weight: 600;
  }
  .tag {
    margin: 4px 0 26px;
    color: var(--muted);
    font-size: 13px;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--muted);
  }
  .none {
    color: var(--muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
  }
  li {
    position: relative;
  }
  li.missing {
    opacity: 0.5;
  }
  .open {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 8px 40px 8px 8px;
    text-align: left;
    border: none;
    border-radius: 8px;
    background: none;
  }
  .open:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.05);
  }
  .still {
    flex: none;
    width: 128px;
    height: 72px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.05);
    overflow: hidden;
  }
  .still img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .what {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .name,
  .meta {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .meta {
    font-size: 12px;
    color: var(--muted);
  }
  .bars {
    margin-top: 5px;
    max-width: 240px;
  }
  .badge {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 9px;
    border-radius: 999px;
    font-size: 12px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--muted);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: currentColor;
  }
  .badge.good {
    color: var(--good);
    background: rgba(127, 201, 138, 0.1);
  }
  .badge.busy {
    color: var(--accent);
    background: rgba(242, 179, 61, 0.1);
  }
  .badge.bad {
    color: var(--bad);
  }
  .x {
    position: absolute;
    right: 6px;
    top: 50%;
    transform: translateY(-50%);
    border: none;
    background: none;
    color: var(--muted);
    padding: 2px 8px;
    opacity: 0;
  }
  li:hover .x,
  .x:focus-visible {
    opacity: 1;
  }
  .right {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .drop {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    border: 2px dashed rgba(242, 179, 61, 0.45);
    border-radius: 14px;
    background: rgba(242, 179, 61, 0.025);
  }
  .drop.dragging {
    border-color: var(--accent);
    background: rgba(242, 179, 61, 0.08);
  }
  .drop svg {
    width: 34px;
    height: 34px;
    fill: none;
    stroke: var(--accent);
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .big {
    font-size: 17px;
  }
  .small {
    color: var(--muted);
    font-size: 12px;
  }
  .cta {
    margin-top: 6px;
    padding: 6px 16px;
    border-radius: 6px;
    background: var(--accent);
    color: #101114;
    font-weight: 600;
  }
  .note {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }
</style>
