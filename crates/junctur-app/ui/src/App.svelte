<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import * as api from "./lib/api";
  import Transport from "./lib/Transport.svelte";
  import ProjectPanel from "./lib/ProjectPanel.svelte";

  let player = $state<api.PlayerState | null>(null);
  let opened = $state<api.Opened | null>(null);
  let edit = $state<api.EditState | null>(null);
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

  const VIDEO = ["mp4", "mkv", "mov", "flv", "ts", "webm", "avi"];

  async function load(path: string) {
    try {
      failure = null;
      opened = await api.openFile(path);
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
    if (e.code === "Space" && player?.path) {
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

<svelte:window onkeydown={onKey} />

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

  <Transport {player} {edit} onOpen={pick} onError={(e) => (failure = e)} />
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
  .failure {
    position: absolute;
    left: 16px;
    top: 16px;
    max-width: 60%;
    padding: 8px 12px;
    border-radius: 6px;
    background: var(--panel);
    border: 1px solid var(--bad);
    color: var(--bad);
  }
</style>
