<script lang="ts">
  // What the app shows with nothing open (2d), in the spirit of Resolve's project chooser: a place
  // to drop or pick a recording, and the recordings opened before. Dropping works anywhere on the
  // window (App.svelte listens); this only draws the target. A new recording opens straight into
  // the editor and is analysed there; nothing here waits on analysis.
  import { onMount } from "svelte";
  import * as api from "./api";

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

  let recent = $state<api.Recent[]>([]);

  onMount(() => {
    api.recent().then((r) => (recent = r), (e) => onError(String(e)));
  });

  async function forget(path: string) {
    try {
      await api.forgetRecent(path);
      recent = recent.filter((r) => r.path !== path);
    } catch (e) {
      onError(String(e));
    }
  }

  /** "today 14:05", "yesterday", "3 Oct". */
  function when(unix: number): string {
    const d = new Date(unix * 1000);
    const now = new Date();
    const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    const days = Math.round((day(now) - day(d)) / 86_400_000);
    if (days === 0) return `today ${d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
    if (days === 1) return "yesterday";
    return d.toLocaleDateString([], { day: "numeric", month: "short", year: d.getFullYear() === now.getFullYear() ? undefined : "numeric" });
  }
</script>

<div class="start">
  <h1>Junctur</h1>
  <div class="columns">
    <button class="drop" class:dragging onclick={onPick} disabled={!ready}>
      <span class="big">Drop a recording here</span>
      <span class="small">or click to choose one. It opens at once and is analysed in the background.</span>
    </button>
    <section class="recent">
      <h2>Recent</h2>
      {#if recent.length === 0}
        <p class="none">Recordings you open show here.</p>
      {:else}
        <ul>
          {#each recent as r (r.path)}
            <li class:missing={r.missing}>
              <button
                class="open"
                disabled={!ready || r.missing}
                title={r.missing ? `${r.path} (not found)` : r.path}
                onclick={() => onOpen(r.path)}
              >
                <span class="name">{r.name}</span>
                <span class="folder">{r.missing ? "not found: " : ""}{r.folder}</span>
              </button>
              <span class="when">{when(r.opened)}</span>
              <button class="x" title="Remove from the list" aria-label="Remove {r.name} from the list" onclick={() => forget(r.path)}
                >×</button
              >
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  </div>
</div>

<style>
  .start {
    position: absolute;
    inset: 0 0 64px 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 20px;
    padding: 24px;
    box-sizing: border-box;
  }
  h1 {
    margin: 0;
    font-weight: 600;
    letter-spacing: 0.02em;
  }
  .columns {
    display: flex;
    gap: 20px;
    width: min(900px, 100%);
    height: min(420px, 70vh);
  }
  .drop {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    border: 2px dashed var(--line);
    border-radius: 10px;
    background: var(--panel);
  }
  .drop.dragging {
    border-color: var(--accent);
  }
  .big {
    font-size: 16px;
  }
  .small {
    color: var(--muted);
    max-width: 260px;
    text-align: center;
  }
  .recent {
    flex: 1.3;
    display: flex;
    flex-direction: column;
    min-width: 0;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--panel);
    padding: 10px 0 6px;
  }
  h2 {
    margin: 0 14px 6px;
    font-size: 13px;
    font-weight: 600;
    color: var(--muted);
  }
  .none {
    margin: 8px 14px;
    color: var(--muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
  }
  li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 8px 0 4px;
  }
  li.missing {
    opacity: 0.5;
  }
  .open {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    text-align: left;
    border: none;
    background: none;
    padding: 6px 10px;
  }
  .name,
  .folder {
    max-width: 100%;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .folder,
  .when {
    color: var(--muted);
    font-size: 12px;
  }
  .when {
    white-space: nowrap;
  }
  .x {
    border: none;
    background: none;
    color: var(--muted);
    padding: 2px 6px;
  }
</style>
