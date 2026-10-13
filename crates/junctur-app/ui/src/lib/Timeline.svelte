<script lang="ts">
  // The whole recording in source time: kept stretches lit, cuts dark, the playhead, click/drag to
  // seek, shift-drag to select a stretch to cut or keep (drawn over the bar, as is a transcript
  // selection). The ranges are painted onto a canvas only when the edit list or the size changes (each
  // pixel column shaded by how much of it is kept, see coverage.ts), and the playhead is a separate
  // element moved by CSS, so a position update never repaints thousands of ranges. Since 2g it is the
  // overview: it outlines the window the track lanes show (`view`).
  import { onMount } from "svelte";
  import * as api from "./api";
  import { coverage } from "./coverage";
  import type { Selection } from "./selection";
  import { toFrame, toSeconds } from "./time";

  let {
    player,
    edit,
    span,
    view = null,
    onSelect,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    span: { start: number; end: number } | null;
    /** The stretch the track lanes show, outlined (source seconds). */
    view?: { start: number; end: number } | null;
    onSelect: (s: Selection | null) => void;
    onError: (message: string) => void;
  } = $props();

  let bar: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let width = $state(0);

  const loaded = $derived(!!player?.path);
  const hasRanges = $derived(!!edit?.fps && edit.ranges.length > 0);
  // Source length: the edit list's frame grid when there is one, else what mpv reports.
  const duration = $derived(
    hasRanges && edit?.fps
      ? toSeconds(edit.totalFrames, edit.fps)
      : (player?.duration ?? 0),
  );

  // While dragging, and until mpv's position catches up with the seek, the bar shows the drag
  // position: mpv's own lags behind seeks, and showing it would make the playhead jump back.
  let scrub = $state<number | null>(null);
  let dragging = false;
  // Keyframe seeks while dragging, at most one per animation frame (pointer events come faster).
  let queued: number | null = null;
  let frame = 0;
  // When to give up waiting for mpv's position to reach the target (a seek that lands elsewhere,
  // e.g. inside a cut while skipping, where it lands on the next kept range).
  let scrubUntil = 0;
  const shown = $derived(scrub ?? player?.position ?? 0);
  const fraction = $derived(duration > 0 ? Math.min(1, Math.max(0, shown / duration)) : 0);

  async function call(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }

  function timeAt(clientX: number): number {
    const r = bar.getBoundingClientRect();
    return Math.min(1, Math.max(0, (clientX - r.left) / r.width)) * duration;
  }

  function dragTo(to: number) {
    scrub = to;
    queued = to;
    if (!frame) {
      frame = requestAnimationFrame(() => {
        frame = 0;
        const t = queued;
        queued = null;
        if (t != null) call(async () => void (await api.seek(t, false)));
      });
    }
  }

  function release(to: number) {
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    queued = null;
    scrub = to;
    scrubUntil = performance.now() + 1500;
    // Hold the playhead where the seek really lands (a cut lands on the next kept range).
    call(async () => {
      const lands = await api.seek(to, true);
      if (scrubUntil !== 0) scrub = lands;
    });
    // If no position update comes at all, let go of the drag position anyway.
    setTimeout(() => {
      if (scrubUntil !== 0 && performance.now() >= scrubUntil) {
        scrub = null;
        scrubUntil = 0;
      }
    }, 1600);
  }

  $effect(() => {
    const pos = player?.position;
    if (scrub == null || dragging || frame || scrubUntil === 0) return;
    if ((pos != null && Math.abs(pos - scrub) < 0.5) || performance.now() > scrubUntil) {
      scrub = null;
      scrubUntil = 0;
    }
  });

  // Shift-drag: a stretch of whole frames from where the drag began.
  let selectFrom: number | null = null;
  function frameAt(clientX: number): number {
    const f = edit?.fps;
    return f ? toFrame(timeAt(clientX), f, Math.round) : 0;
  }

  function selectTo(clientX: number) {
    if (selectFrom == null) return;
    const to = frameAt(clientX);
    const [a, b] = [Math.min(selectFrom, to), Math.max(selectFrom, to)];
    onSelect(b > a ? { kind: "frames", start: a, end: b } : null);
  }

  function down(e: PointerEvent) {
    if (!loaded || !duration || e.button !== 0) return;
    bar.setPointerCapture(e.pointerId);
    if (e.shiftKey && edit?.editable && edit.fps) {
      selectFrom = frameAt(e.clientX);
      return;
    }
    dragging = true;
    dragTo(timeAt(e.clientX));
  }

  function move(e: PointerEvent) {
    if (selectFrom != null) selectTo(e.clientX);
    else if (dragging) dragTo(timeAt(e.clientX));
  }

  function up(e: PointerEvent) {
    if (selectFrom != null) {
      selectTo(e.clientX);
      selectFrom = null;
      return;
    }
    if (!dragging) return;
    dragging = false;
    release(timeAt(e.clientX));
  }

  function paint() {
    const ctx = canvas?.getContext("2d");
    if (!ctx) return;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(width * dpr));
    const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
    canvas.width = w;
    canvas.height = h;
    const css = getComputedStyle(canvas);
    ctx.clearRect(0, 0, w, h);
    if (!hasRanges || !edit) {
      ctx.fillStyle = css.getPropertyValue("--track");
      ctx.fillRect(0, 0, w, h);
      return;
    }
    ctx.fillStyle = css.getPropertyValue("--cut");
    ctx.fillRect(0, 0, w, h);
    ctx.fillStyle = css.getPropertyValue("--kept");
    const kept = coverage(edit.ranges, edit.totalFrames, w);
    for (let x = 0; x < w; x++) {
      if (kept[x] <= 0) continue;
      ctx.globalAlpha = Math.min(1, kept[x]);
      ctx.fillRect(x, 0, 1, h);
    }
    ctx.globalAlpha = 1;
  }

  $effect(() => {
    // Repaint when the edit list or the size changes, never for a position.
    void edit;
    void width;
    paint();
  });

  onMount(() => {
    const ro = new ResizeObserver(() => (width = bar.clientWidth));
    ro.observe(bar);
    return () => ro.disconnect();
  });
</script>

<div
  class="bar"
  class:disabled={!loaded || !duration}
  bind:this={bar}
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  role="slider"
  tabindex="-1"
  aria-label="Position in the recording"
  aria-valuemin={0}
  aria-valuemax={duration}
  aria-valuenow={shown}
>
  <canvas bind:this={canvas}></canvas>
  {#if span && duration}
    <div
      class="sel"
      style:left="{(Math.max(0, span.start) / duration) * 100}%"
      style:width="{(Math.max(0, span.end - span.start) / duration) * 100}%"
    ></div>
  {/if}
  {#if view && duration && view.end - view.start < duration * 0.999}
    <div
      class="view"
      style:left="{(Math.max(0, view.start) / duration) * 100}%"
      style:width="{(Math.max(0, Math.min(duration, view.end) - Math.max(0, view.start)) / duration) * 100}%"
    ></div>
  {/if}
  {#if loaded && duration}
    <div class="head" style:left="{fraction * 100}%"></div>
  {/if}
</div>

<style>
  .bar {
    position: relative;
    flex: 1;
    min-width: 80px;
    height: 22px;
    cursor: pointer;
    touch-action: none;
  }
  .bar.disabled {
    cursor: default;
    opacity: 0.4;
  }
  canvas {
    --track: rgba(255, 255, 255, 0.18);
    --cut: rgba(255, 255, 255, 0.1);
    --kept: rgba(242, 179, 61, 0.75);
    position: absolute;
    left: 0;
    right: 0;
    top: 6px;
    width: 100%;
    height: 10px;
    border-radius: 3px;
  }
  .sel {
    position: absolute;
    top: 3px;
    height: 16px;
    min-width: 2px;
    border-radius: 2px;
    background: rgba(120, 170, 255, 0.35);
    border: 1px solid rgba(120, 170, 255, 0.8);
    box-sizing: border-box;
    pointer-events: none;
  }
  .view {
    position: absolute;
    top: 2px;
    height: 18px;
    min-width: 4px;
    border: 1.5px solid var(--accent);
    border-radius: 3px;
    box-sizing: border-box;
    pointer-events: none;
  }
  .head {
    position: absolute;
    top: 1px;
    width: 2px;
    height: 20px;
    margin-left: -1px;
    border-radius: 1px;
    background: var(--text);
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.5);
    pointer-events: none;
  }
</style>
