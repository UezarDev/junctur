<script lang="ts">
  // The track lanes under the play bar (2g, mock B/LV1): one lane per audio track on one time
  // axis, with the Speakers lane on top when the edit list names speakers. Each lane's header has
  // its colour, name, what it is, Mute, Solo and a meter; the body shows a zoomed window of the
  // recording (`view`) with a ruler: the waveforms, the speakers' turns, the cuts shaded across
  // every lane, the selection, and one playhead through all of them.
  //
  // The bodies are one canvas, painted only when what they show changes (the window, the size, the
  // waveforms read so far, the edit list, the turns, the mutes), never for a position: the playhead
  // and the selection are elements moved by CSS. Waveforms are asked of the engine one column per
  // pixel of the window shown (`api.waves`), so the page never holds a 30 h recording's peaks.
  //
  // A click on a lane seeks, a drag selects a stretch to cut or keep (whole frames, as the
  // timeline's shift-drag does), a drag on the ruler scrubs, the wheel pans and Ctrl+wheel zooms
  // around the pointer.
  import { onMount } from "svelte";
  import * as api from "./api";
  import Avatars from "./Avatars.svelte";
  import MuteSolo from "./MuteSolo.svelte";
  import { RULER } from "./layout";
  import { meter } from "./meters";
  import type { Selection } from "./selection";
  import type { Talk } from "./talking";
  import { clock, toFrame } from "./time";
  import * as tr from "./tracks";

  let {
    player,
    edit,
    mixer,
    talk,
    talking,
    view,
    row,
    speakersRow,
    head,
    thin,
    wavesVersion,
    span,
    onView,
    onSelect,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    mixer: api.MixerState | null;
    talk: Talk;
    talking: Set<number>;
    /** The window shown, in source seconds. */
    view: { start: number; len: number };
    row: number;
    speakersRow: number;
    head: number;
    /** Portrait's thin lanes: no second line in the headers. */
    thin: boolean;
    /** Moves when more of the waveforms has been read. */
    wavesVersion: number;
    span: { start: number; end: number } | null;
    onView: (start: number, len: number) => void;
    onSelect: (s: Selection | null) => void;
    onError: (message: string) => void;
  } = $props();

  let body: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let width = $state(0);
  let height = $state(0);

  const tracks = $derived(mixer?.tracks ?? []);
  const speakers = $derived(edit?.speakers ?? []);
  const audio = $derived(player?.audioTracks ?? []);
  const duration = $derived(player?.duration ?? 0);
  const end = $derived(view.start + view.len);
  // What the lanes need: below the height left they scroll (headers and bodies together).
  const content = $derived(RULER + speakersRow + tracks.length * row);

  const x = (t: number) => ((t - view.start) / view.len) * width;
  const timeAt = (clientX: number) => {
    const r = body.getBoundingClientRect();
    return view.start + ((clientX - r.left) / Math.max(1, r.width)) * view.len;
  };

  // The waveforms of the window shown, fetched when it, the size or what was read changes; one
  // request at a time, the latest wanted one sent when it returns.
  let peaks = $state.raw<api.WavesView | null>(null);
  let inFlight = false;
  let again = false;
  async function fetchWaves() {
    if (inFlight) {
      again = true;
      return;
    }
    inFlight = true;
    try {
      const cols = Math.max(1, Math.round(width));
      peaks = width > 0 && tracks.length ? await api.waves(view.start, view.start + view.len, cols) : null;
    } catch (e) {
      onError(String(e));
    } finally {
      inFlight = false;
      if (again) {
        again = false;
        void fetchWaves();
      }
    }
  }
  $effect(() => {
    void view.start;
    void view.len;
    void width;
    void wavesVersion;
    void tracks.length;
    void fetchWaves();
  });

  let hatch: CanvasPattern | null = null;
  function pattern(ctx: CanvasRenderingContext2D, dpr: number): CanvasPattern | null {
    if (hatch) return hatch;
    const n = Math.round(8 * dpr);
    const c = document.createElement("canvas");
    c.width = c.height = n;
    const g = c.getContext("2d")!;
    g.strokeStyle = "rgba(239, 111, 94, 0.22)";
    g.lineWidth = dpr;
    g.beginPath();
    g.moveTo(0, n);
    g.lineTo(n, 0);
    g.stroke();
    hatch = ctx.createPattern(c, "repeat");
    return hatch;
  }

  /** A ruler step giving labels roughly every 90 px. */
  function step(len: number, w: number): number {
    const want = (len / Math.max(1, w)) * 90;
    const steps = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 14400];
    return steps.find((s) => s >= want) ?? 28800;
  }

  function paint() {
    const ctx = canvas?.getContext("2d");
    if (!ctx || width <= 0 || height <= 0) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);
    const css = getComputedStyle(canvas);
    const line = css.getPropertyValue("--line").trim();
    const muted = css.getPropertyValue("--muted").trim();

    // Ruler.
    ctx.fillStyle = muted;
    ctx.font = "11px 'Segoe UI', system-ui, sans-serif";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    const s = step(view.len, width);
    for (let t = Math.ceil(view.start / s) * s; t < end; t += s) {
      const px = x(t);
      ctx.fillRect(Math.round(px), RULER - 5, 1, 5);
      if (px > 18 && px < width - 18) ctx.fillText(clock(t), px, RULER / 2 - 1);
    }
    ctx.fillStyle = line;
    ctx.fillRect(0, RULER - 1, width, 1);

    // Speakers lane: a row per speaker, their turns as bars.
    let y = RULER;
    if (speakersRow > 0 && speakers.length) {
      const n = speakers.length;
      const rowH = (speakersRow - 8) / n;
      const index = new Map(speakers.map((sp, i) => [sp.id, i]));
      talk.each(view.start, end, (a, b, who) => {
        const i = index.get(who);
        if (i === undefined) return;
        ctx.fillStyle = speakers[i].color;
        const x0 = Math.max(0, x(a));
        const x1 = Math.min(width, x(b));
        const h = Math.max(3, Math.min(5, rowH - 2));
        ctx.fillRect(x0, y + 4 + i * rowH + (rowH - h) / 2, Math.max(1.5, x1 - x0), h);
      });
      y += speakersRow;
      ctx.fillStyle = line;
      ctx.fillRect(0, y - 1, width, 1);
    }

    // Track lanes: each waveform mirrored about the lane's middle, in the track's colour, dimmer
    // when it is not heard.
    const cols = peaks?.columns ?? 0;
    tracks.forEach((t, k) => {
      const top = y + k * row;
      const mid = top + row / 2;
      const half = row / 2 - 3;
      ctx.fillStyle = tr.colour(t, speakers);
      ctx.globalAlpha = tr.heard(t, tracks) ? 0.85 : 0.4;
      if (peaks && cols > 0 && k < peaks.tracks) {
        const step = width / cols;
        for (let c = 0; c < cols; c++) {
          const v = peaks.peaks[k * cols + c];
          if (!v) continue;
          const h = Math.max(0.5, (v / 255) * half);
          ctx.fillRect(c * step, mid - h, Math.max(1, step - 0.25), 2 * h);
        }
      }
      // The centre line, where there is no sound.
      ctx.globalAlpha = 0.25;
      ctx.fillRect(0, mid - 0.5, width, 1);
      ctx.globalAlpha = 1;
      ctx.fillStyle = line;
      ctx.fillRect(0, top + row - 1, width, 1);
    });

    // Cuts across every lane (not the ruler): what the edit list does not keep.
    if (edit?.fps && edit.ranges.length) {
      const fps = edit.fps;
      const sec = (f: number) => (f * fps.den) / fps.num;
      const r = edit.ranges;
      const n = r.length / 2;
      const shade = (a: number, b: number) => {
        const x0 = Math.max(0, x(a));
        const x1 = Math.min(width, x(b));
        if (x1 <= x0) return;
        ctx.fillStyle = "rgba(0, 0, 0, 0.38)";
        ctx.fillRect(x0, RULER, Math.max(1, x1 - x0), height - RULER);
        const p = pattern(ctx, dpr);
        if (p) {
          ctx.fillStyle = p;
          ctx.fillRect(x0, RULER, Math.max(1, x1 - x0), height - RULER);
        }
      };
      // The first range ending after the window's start.
      let lo = 0;
      let hi = n;
      while (lo < hi) {
        const m = (lo + hi) >> 1;
        if (sec(r[2 * m + 1]) <= view.start) lo = m + 1;
        else hi = m;
      }
      let from = lo > 0 ? sec(r[2 * lo - 1]) : 0;
      for (let i = lo; i < n && from < end; i++) {
        shade(from, sec(r[2 * i]));
        from = sec(r[2 * i + 1]);
      }
      if (from < end) shade(from, sec(edit.totalFrames));
    }
  }

  $effect(() => {
    // Repaint when anything drawn changes, never for a position.
    void peaks;
    void edit;
    void tracks;
    void speakers;
    void talk;
    void view.start;
    void view.len;
    void width;
    void height;
    void row;
    void speakersRow;
    paint();
  });

  // Pointer: a click seeks, a drag on a lane selects, a drag on the ruler scrubs.
  let down: { x: number; t: number; ruler: boolean; moved: boolean } | null = null;
  let queued: number | null = null;
  let frame = 0;
  async function call(f: () => Promise<unknown>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }
  function scrub(t: number) {
    queued = t;
    if (!frame) {
      frame = requestAnimationFrame(() => {
        frame = 0;
        const q = queued;
        queued = null;
        if (q != null) void call(() => api.seek(q, false));
      });
    }
  }
  const clampT = (t: number) => Math.min(Math.max(0, t), duration || t);
  function select(a: number, b: number) {
    const fps = edit?.fps;
    if (!fps || !edit?.editable) return;
    const [p, q] = [toFrame(clampT(Math.min(a, b)), fps, Math.round), toFrame(clampT(Math.max(a, b)), fps, Math.round)];
    onSelect(q > p ? { kind: "frames", start: p, end: q } : null);
  }
  function pointerDown(e: PointerEvent) {
    if (e.button !== 0 || !player?.path) return;
    body.setPointerCapture(e.pointerId);
    const r = body.getBoundingClientRect();
    down = { x: e.clientX, t: timeAt(e.clientX), ruler: e.clientY - r.top < RULER, moved: false };
  }
  function pointerMove(e: PointerEvent) {
    if (!down) return;
    if (Math.abs(e.clientX - down.x) > 3) down.moved = true;
    if (!down.moved) return;
    if (down.ruler) scrub(clampT(timeAt(e.clientX)));
    else select(down.t, timeAt(e.clientX));
  }
  function pointerUp(e: PointerEvent) {
    if (!down) return;
    const d = down;
    down = null;
    if (d.ruler || !d.moved) {
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
      void call(() => api.seek(clampT(timeAt(e.clientX)), true));
      if (!d.ruler) onSelect(null);
    } else {
      select(d.t, timeAt(e.clientX));
    }
  }
  function wheel(e: WheelEvent) {
    if (!duration) return;
    e.preventDefault();
    if (e.ctrlKey) {
      const at = timeAt(e.clientX);
      const len = Math.min(duration, Math.max(5, view.len * Math.pow(1.0015, e.deltaY)));
      const share = (at - view.start) / view.len;
      onView(at - share * len, len);
    } else {
      const d = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
      onView(view.start + (d / Math.max(1, width)) * view.len, view.len);
    }
  }

  const position = $derived(player?.position ?? null);
  const headX = $derived(position == null ? null : x(position));
  const sel = $derived(span ? { a: Math.max(0, x(span.start)), b: Math.min(width, x(span.end)) } : null);

  onMount(() => {
    const ro = new ResizeObserver(() => {
      width = body.clientWidth;
      height = body.clientHeight;
    });
    ro.observe(body);
    return () => ro.disconnect();
  });
</script>

<div class="lanes" class:thin style:--head="{head}px">
  <div class="inner" style:height="{content}px">
  <div class="heads">
    <div class="h ruler" style:height="{RULER}px">
      <span>Tracks</span>
      <span class="tag" title="Every track plays at once; mute or solo any of them (preview only)">all play together</span>
    </div>
    {#if speakersRow > 0}
      <div class="h speakers" style:height="{speakersRow}px">
        <span class="txt">
          <span class="nm">Speakers</span>
          {#if !thin}<span class="sub">live · from analysis</span>{/if}
        </span>
        <Avatars {speakers} {talking} {mixer} size="small" />
      </div>
    {/if}
    {#each tracks as t (t.index)}
      <div class="h track" style:height="{row}px" title={tr.title(t)}>
        <span class="dot" style:background={tr.colour(t, speakers)}></span>
        <span class="txt">
          <span class="nm">{t.index + 1} · {tr.name(t, audio)}</span>
          {#if !thin && row >= 34}<span class="sub">{tr.describe(t, audio, speakers)}</span>{/if}
        </span>
        <MuteSolo track={t} kind="mute" {onError} />
        <MuteSolo track={t} kind="solo" {onError} />
        <span class="meter" class:off={!tr.heard(t, tracks)}>
          <span class="bar" use:meter={{ track: t.index, axis: "y" }}></span>
        </span>
      </div>
    {/each}
  </div>
  <div
    class="body"
    bind:this={body}
    onpointerdown={pointerDown}
    onpointermove={pointerMove}
    onpointerup={pointerUp}
    onpointercancel={() => (down = null)}
    onwheel={wheel}
    role="slider"
    tabindex="-1"
    aria-label="Track lanes: click to seek, drag to select"
    aria-valuemin={0}
    aria-valuemax={duration}
    aria-valuenow={position ?? 0}
  >
    <canvas bind:this={canvas}></canvas>
    {#if sel && sel.b > sel.a}
      <div class="sel" style:left="{sel.a}px" style:width="{sel.b - sel.a}px"></div>
    {/if}
    {#if headX != null && headX >= 0 && headX <= width}
      <div class="head" style:transform="translateX({headX}px)"></div>
    {/if}
  </div>
  </div>
</div>

<style>
  /* Scrolls when the lanes need more than the height left (many tracks, a short window): the
     headers and the bodies move together. The wheel over the headers scrolls; over the bodies it
     pans time. */
  .lanes {
    height: 100%;
    overflow-x: hidden;
    overflow-y: auto;
    background: var(--panel-solid);
    border-top: 1px solid var(--line);
    font-size: 13px;
  }
  .inner {
    display: flex;
    min-height: 100%;
  }
  .heads {
    flex: none;
    width: var(--head);
    border-right: 1px solid var(--line);
    overflow: hidden;
  }
  .h {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 6px 0 8px;
    box-sizing: border-box;
    border-bottom: 1px solid var(--line);
  }
  .h.ruler {
    font-size: 11px;
    color: var(--muted);
  }
  .tag {
    padding: 0 6px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.07);
    white-space: nowrap;
  }
  .txt {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    line-height: 1.15;
  }
  .nm,
  .sub {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sub {
    font-size: 11px;
    color: var(--muted);
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }
  .meter {
    flex: none;
    position: relative;
    width: 5px;
    height: calc(100% - 10px);
    min-height: 10px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }
  .bar {
    position: absolute;
    inset: 0;
    background: var(--good);
    transform: scaleY(0);
    transition: transform 80ms linear;
  }
  .meter.off .bar {
    background: var(--muted);
  }
  .thin .h.track {
    gap: 4px;
  }
  .body {
    position: relative;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    cursor: text;
    touch-action: none;
  }
  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }
  .sel {
    position: absolute;
    top: 22px;
    bottom: 0;
    background: rgba(120, 170, 255, 0.18);
    border-left: 1px solid rgba(120, 170, 255, 0.85);
    border-right: 1px solid rgba(120, 170, 255, 0.85);
    pointer-events: none;
  }
  .head {
    position: absolute;
    top: 0;
    bottom: 0;
    left: -1px;
    width: 2px;
    background: #fff;
    pointer-events: none;
  }
  .head::before {
    content: "";
    position: absolute;
    top: 0;
    left: -4px;
    border: 5px solid transparent;
    border-top: 6px solid #fff;
  }
</style>
