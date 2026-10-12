<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import * as api from "./lib/api";
  import Transport from "./lib/Transport.svelte";
  import ProjectPanel from "./lib/ProjectPanel.svelte";
  import TranscriptPanel from "./lib/TranscriptPanel.svelte";
  import * as sel from "./lib/selection";
  import { words as toWords, type Words } from "./lib/transcript";

  // Raw (not deeply reactive): these are replaced whole on every update, and a 30 h transcript or
  // edit list read through a reactive proxy makes every layout pass several times slower.
  let player = $state.raw<api.PlayerState | null>(null);
  let opened = $state<api.Opened | null>(null);
  let edit = $state.raw<api.EditState | null>(null);
  // The project panel shows the edit list as last read (it reloads when the file changes).
  const project = $derived(
    opened?.project && edit
      ? {
          ...opened.project,
          edit: edit.summary,
          editError: edit.summary ? null : edit.error,
        }
      : (opened?.project ?? null),
  );
  // Set when the player could not start (e.g. libmpv missing) or a command failed.
  let failure = $state<string | null>(null);
  let dragging = $state(false);

  // The transcript and what is selected in it (or on the timeline).
  let words = $state.raw<Words | null>(null);
  let wordsLoading = $state(false);
  let selection = $state<sel.Selection | null>(null);
  let panelOpen = $state(true);
  let windowWidth = $state(window.innerWidth);
  const PANEL = 400;
  const panelWidth = $derived(player?.path && panelOpen ? Math.min(PANEL, windowWidth * 0.45) : 0);
  const span = $derived(sel.seconds(selection, words, edit?.fps ?? null));

  // The panel is opaque; mpv keeps the picture out from under it. Sent once a window resize
  // settles, and only when the share really changed (each is a blocking mpv call).
  let sentShare = -1;
  let marginTimer = 0;
  $effect(() => {
    const share = windowWidth > 0 ? Math.round((panelWidth / windowWidth) * 1000) / 1000 : 0;
    clearTimeout(marginTimer);
    marginTimer = window.setTimeout(() => {
      if (share === sentShare) return;
      sentShare = share;
      api.setVideoMargin(share).catch(() => (sentShare = -1));
    }, 100);
  });

  async function loadTranscript() {
    words = null;
    selection = null;
    wordsLoading = true;
    try {
      const t0 = performance.now();
      const view = await api.transcript();
      const t1 = performance.now();
      words = view ? toWords(view) : null;
      const n = words?.text.length ?? 0;
      void api.uiLog(
        `transcript ${n} words: fetched in ${(t1 - t0).toFixed(0)} ms, split in ${(performance.now() - t1).toFixed(0)} ms`,
      );
    } catch (e) {
      failure = String(e);
    } finally {
      wordsLoading = false;
    }
  }

  const VIDEO = ["mp4", "mkv", "mov", "flv", "ts", "webm", "avi"];

  async function load(path: string) {
    try {
      failure = null;
      opened = await api.openFile(path);
      await loadTranscript();
    } catch (e) {
      failure = String(e);
    }
  }

  async function pick() {
    try {
      const path = await open({ multiple: false, filters: [{ name: "Video", extensions: VIDEO }] });
      if (typeof path === "string") await load(path);
    } catch (e) {
      failure = String(e);
    }
  }

  async function run(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      failure = String(e);
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
    const ctrl = e.ctrlKey || e.metaKey;
    if (edit?.editable && player?.path) {
      if (ctrl && (e.code === "KeyY" || (e.code === "KeyZ" && e.shiftKey))) {
        e.preventDefault();
        return run(api.redo);
      }
      if (ctrl && e.code === "KeyZ") {
        e.preventDefault();
        return run(api.undo);
      }
      if (selection && (e.code === "Delete" || e.code === "Backspace")) {
        e.preventDefault();
        const s = selection;
        return run(() => sel.apply(s, false));
      }
      if (selection && e.code === "KeyK" && !ctrl) {
        const s = selection;
        return run(() => sel.apply(s, true));
      }
    }
    if (e.code === "Escape") {
      selection = null;
    } else if (e.code === "KeyT" && !ctrl && player?.path) {
      panelOpen = !panelOpen;
    } else if (e.code === "Space" && player?.path) {
      e.preventDefault();
      run(api.togglePause);
    } else if (e.code === "KeyS" && player?.path && edit?.ranges.length) {
      run(() => api.setSkip(!edit?.skip));
    }
  }

  onMount(() => {
    api.playerState().then(
      async (s) => {
        player = s;
        edit = await api.editState();
        const file = await api.startupFile();
        if (file) await load(file);
      },
      (e) => (failure = String(e)),
    );
    const unlisten = [
      api.onPlayerState((s) => (player = s)),
      api.onEditState((s) => (edit = s)),
      getCurrentWebview().onDragDropEvent((e) => {
        const p = e.payload;
        dragging = p.type === "enter" || p.type === "over";
        if (p.type === "drop" && p.paths.length > 0) load(p.paths[0]);
      }),
    ];
    return () => unlisten.forEach((u) => u.then((f) => f()));
  });
</script>

<svelte:window onkeydown={onKey} onresize={() => (windowWidth = window.innerWidth)} />

<main class:dragging>
  {#if !player?.path}
    <div class="empty">
      <h1>Junctur</h1>
      <p>Drop a recording here, or</p>
      <button onclick={pick} disabled={!player}>Open a recording</button>
    </div>
  {/if}

  {#if opened}
    <ProjectPanel status={project} error={opened.projectError} />
  {/if}

  {#if failure || player?.error}
    <div class="failure" role="alert">{failure ?? player?.error}</div>
  {/if}

  {#if panelWidth > 0}
    <div class="side" style:width="{panelWidth}px">
      <TranscriptPanel
        {words}
        loading={wordsLoading}
        {player}
        {edit}
        {selection}
        onSelect={(s) => (selection = s)}
        onError={(e) => (failure = e)}
      />
    </div>
  {/if}

  <Transport
    {player}
    {edit}
    {selection}
    {span}
    right={panelWidth}
    onSelect={(s) => (selection = s)}
    onOpen={pick}
    onError={(e) => (failure = e)}
  />
</main>

<style>
  main {
    position: relative;
    height: 100%;
    box-sizing: border-box;
    outline: 2px dashed transparent;
    outline-offset: -10px;
    transition: outline-color 120ms;
  }
  main.dragging {
    outline-color: var(--accent);
  }
  .empty {
    position: absolute;
    inset: 0 0 64px 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
  }
  .empty h1 {
    margin: 0 0 8px;
    font-weight: 600;
    letter-spacing: 0.02em;
  }
  .empty p {
    margin: 0 0 8px;
    color: var(--muted);
  }
  .side {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
  }
  .failure {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    top: 16px;
    max-width: 60%;
    padding: 8px 12px;
    border-radius: 6px;
    background: var(--panel);
    border: 1px solid var(--bad);
    color: var(--bad);
  }
</style>
