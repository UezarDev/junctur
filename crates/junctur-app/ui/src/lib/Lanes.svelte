<script lang="ts">
  // The track lanes under the play bar: one lane per audio track on one time axis, painted on one
  // canvas only when what they show changes; waveforms are fetched per pixel column of the window.
  import { onMount, untrack } from "svelte";
  import * as api from "./api";
  import Avatars from "./Avatars.svelte";
  import MuteSolo from "./MuteSolo.svelte";
  import { RULER } from "./layout";
  import { meter } from "./meters";
  import type { Selection } from "./selection";
  import type { Talk } from "./talking";
  import { clampEdge, dragged, edgeNear, type Edge } from "./nudge";
  import { clock, timecode, toFrame, toSeconds } from "./time";
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
    spans,
    edges = true,
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
    /** The selection, a span per stretch. */
    spans: { start: number; end: number }[];
    /** Cut edges can be dragged (not while reviewing: review moves its clip's edges itself). */
    edges?: boolean;
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
    // --bad (#f25f8f) written out: a canvas colour cannot read a CSS token.
    g.strokeStyle = "rgba(242, 95, 143, 0.22)";
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
      // Every cut's edge in view, drawn at once, so an edge is seen before it is hovered; the one
      // in reach lights up above.
      if (edges && edit.editable) {
        // A light tint of --accent written out (the canvas cannot read the token).
        ctx.fillStyle = "rgba(255, 160, 140, 0.5)";
        for (let i = lo; i < n; i++) {
          if (sec(r[2 * i]) > end) break;
          for (const f of [r[2 * i], r[2 * i + 1]]) {
            if (f <= 0 || f >= edit.totalFrames) continue;
            const px = x(sec(f));
            if (px >= 0 && px <= width) ctx.fillRect(Math.round(px) - 1, RULER, 2, height - RULER);
          }
        }
      }
    }
  }

  $effect(() => {
    // Repaint when anything drawn changes, never for a position.
    void peaks;
    void edit;
    void edges;
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
  // Cut edges: within GRAB px of one the press drags it. The edit waits for the release, so a drag
  // is one undo step, then the preview plays from PREROLL before where the edge landed.
  const GRAB = 8;
  const PREROLL = 1.5;
  let hover = $state.raw<Edge | null>(null);
  let drag = $state.raw<{ edge: Edge; to: number } | null>(null);
  // Where the pointer last was during a drag, so a zoom or pan under it moves the edge with it.
  let dragX = 0;
  // A drag's edit sent and its new list not yet here: no edge is offered until it is, since one
  // grabbed from the old list would be placed by frames that have moved. Cleared by the next edit
  // list, by a failure, or after WAIT ms if no list comes (the edit changed nothing).
  let waiting = false;
  let waitTimer = 0;
  const WAIT = 2000;
  function stopWaiting() {
    waiting = false;
    clearTimeout(waitTimer);
  }
  function edgeAt(clientX: number, clientY: number): Edge | null {
    const fps = edit?.fps;
    if (waiting || !edges || !fps || !edit?.editable || !edit.ranges.length || width <= 0) return null;
    // One read of the box per pointer move.
    const r = body.getBoundingClientRect();
    if (clientY - r.top < RULER) return null;
    const perSecond = fps.num / fps.den;
    const t = view.start + ((clientX - r.left) / Math.max(1, r.width)) * view.len;
    return edgeNear(edit, t * perSecond, (GRAB * view.len * perSecond) / width);
  }
  const sameEdge = (a: Edge | null, b: Edge | null) => a === b || (!!a && !!b && a.at === b.at && a.frame === b.frame);
  function setHover(g: Edge | null) {
    if (!sameEdge(g, hover)) hover = g;
  }
  // The dragged edge under `clientX`, assigned only when it lands on another frame.
  function dragTo(clientX: number) {
    const fps = edit?.fps;
    if (!drag || !fps) return;
    dragX = clientX;
    const to = clampEdge(drag.edge, (timeAt(clientX) * fps.num) / fps.den);
    if (to !== drag.to) drag = { edge: drag.edge, to };
  }
  // A handle shown for an edit list or a window that has since changed would be in the wrong place.
  $effect(() => {
    void edit;
    void view.start;
    void view.len;
    void edges;
    hover = null;
  });
  // A new edit list ends the wait for a drag's edit.
  $effect(() => {
    void edit;
    untrack(stopWaiting);
  });
  // The wheel zooms and pans while dragging too: the edge follows the pointer in the new window.
  $effect(() => {
    void view.start;
    void view.len;
    void width;
    untrack(() => {
      if (drag) dragTo(dragX);
    });
  });
  function release(d: { edge: Edge; to: number }, clientX: number) {
    const fps = edit?.fps;
    if (!edit || !fps) return;
    if (d.to === d.edge.frame) {
      // Let go where it started: a click, which seeks.
      void call(() => api.seek(clampT(timeAt(clientX)), true));
      onSelect(null);
      return;
    }
    // The same edge with the same room: a list that changed elsewhere (an undo, an outside edit)
    // would make the frames dragged over mean something else.
    const live = edgeNear(edit, d.edge.frame, 0);
    if (!live || live.at !== d.edge.at || live.lo !== d.edge.lo || live.hi !== d.edge.hi) {
      onError("The cuts changed while the edge was being dragged; nothing was moved. Drag it again.");
      return;
    }
    const step = dragged(edit, d.edge, d.to);
    if (step) {
      const at = toSeconds(clampEdge(d.edge, d.to), fps);
      waiting = true;
      clearTimeout(waitTimer);
      waitTimer = window.setTimeout(stopWaiting, WAIT);
      void (async () => {
        try {
          // The words beside the edge move with it.
          await api.moveEdge(step, d.edge.frame);
        } catch (e) {
          stopWaiting();
          onError(String(e));
          return;
        }
        await call(() => api.playFrom(Math.max(0, at - PREROLL)));
      })();
    }
  }

  function pointerDown(e: PointerEvent) {
    if (e.button !== 0 || !player?.path) return;
    body.setPointerCapture(e.pointerId);
    const g = edgeAt(e.clientX, e.clientY);
    if (g) {
      drag = { edge: g, to: g.frame };
      dragX = e.clientX;
      hover = null;
      return;
    }
    const r = body.getBoundingClientRect();
    down = { x: e.clientX, t: timeAt(e.clientX), ruler: e.clientY - r.top < RULER, moved: false };
  }
  function pointerMove(e: PointerEvent) {
    if (drag) {
      dragTo(e.clientX);
      return;
    }
    if (!down) {
      setHover(edgeAt(e.clientX, e.clientY));
      return;
    }
    if (Math.abs(e.clientX - down.x) > 3) down.moved = true;
    if (!down.moved) return;
    if (down.ruler) scrub(clampT(timeAt(e.clientX)));
    else select(down.t, timeAt(e.clientX));
  }
  function pointerUp(e: PointerEvent) {
    if (drag) {
      const d = drag;
      drag = null;
      release(d, e.clientX);
      setHover(edgeAt(e.clientX, e.clientY));
      return;
    }
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
  // The handle (hovered) or the edge being dragged, with its timecode.
  const grab = $derived.by(() => {
    const fps = edit?.fps;
    if (!fps) return null;
    const f = drag ? drag.to : hover?.frame;
    if (f == null) return null;
    const px = x(toSeconds(f, fps));
    return px >= 0 && px <= width ? { px, label: drag ? timecode(f, fps) : null } : null;
  });
  const sel = $derived(
    spans.map((s) => ({ a: Math.max(0, x(s.start)), b: Math.min(width, x(s.end)) })).filter((s) => s.b > s.a),
  );

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
    onpointercancel={() => {
      down = null;
      drag = null;
    }}
    onpointerleave={() => {
      if (!drag) hover = null;
    }}
    onwheel={wheel}
    class:grabbing={!!grab}
    role="slider"
    tabindex="-1"
    aria-label="Track lanes: click to seek, drag to select, drag a cut's edge to move it"
    aria-valuemin={0}
    aria-valuemax={duration}
    aria-valuenow={position ?? 0}
  >
    <canvas bind:this={canvas}></canvas>
    {#each sel as s, k (k)}
      <div class="sel" style:left="{s.a}px" style:width="{s.b - s.a}px"></div>
    {/each}
    {#if grab}
      <div class="edge" class:on={!!drag} style:top="{RULER}px" style:transform="translateX({grab.px}px)">
        {#if grab.label}<span class="when mono" class:flip={grab.px > width - 96}>{grab.label}</span>{/if}
      </div>
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
  .body.grabbing {
    cursor: ew-resize;
  }
  /* A cut's edge under the pointer, or being dragged; lit over the thin mark every edge has. */
  .edge {
    position: absolute;
    bottom: 0;
    left: -2px;
    width: 4px;
    border-radius: 2px;
    background: color-mix(in srgb, color-mix(in srgb, var(--accent) 55%, white) 85%, transparent);
    box-shadow: 0 0 6px color-mix(in srgb, var(--accent) 60%, transparent);
    pointer-events: none;
  }
  .edge.on {
    background: color-mix(in srgb, var(--accent) 40%, white);
  }
  .when {
    position: absolute;
    top: 4px;
    left: 8px;
    padding: 1px 6px;
    border-radius: 999px;
    background: rgba(16, 17, 20, 0.92);
    color: #fff;
    font-size: 11px;
    white-space: nowrap;
  }
  .when.flip {
    left: auto;
    right: 8px;
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
