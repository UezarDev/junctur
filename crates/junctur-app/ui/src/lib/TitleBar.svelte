<script lang="ts">
  // Junctur's own title bar (2i): the window is frameless, so this is the whole caption. The app
  // icon, File and View as icon buttons with dropdowns (menus.ts), the Settings gear, a drag area
  // (Tauri's `data-tauri-drag-region`: a press drags through Windows' own move loop, so snapping to
  // an edge works; a double press maximises), then minimise, maximise and close. It is opaque and
  // mpv keeps the picture below it (App.svelte's top margin).
  //
  // Snap layouts: Windows shows them only for a native maximise button, which this is not, so
  // resting on it asks the app for them (src/window.rs).
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import icon from "../../../icons/32x32.png";
  import * as api from "./api";
  import { TITLE } from "./layout";
  import { MENUS, checked, enabled, type Menu, type MenuId, type MenuState } from "./menus";

  let { menu, onMenu }: { menu: MenuState; onMenu: (id: MenuId) => void } = $props();

  const win = getCurrentWindow();
  let open = $state<Menu["id"] | null>(null);
  let maximized = $state(false);
  let bar: HTMLElement;

  // A modal dialog took over: an open dropdown goes.
  $effect(() => {
    if (menu.locked) open = null;
  });

  function choose(id: MenuId) {
    open = null;
    onMenu(id);
  }

  // Resting on maximise for half a second, as Windows waits on its own button.
  const SNAP_AFTER = 500;
  let snapTimer = 0;
  function rest() {
    clearTimeout(snapTimer);
    snapTimer = window.setTimeout(() => void api.snapLayouts().catch(() => {}), SNAP_AFTER);
  }
  function leave() {
    clearTimeout(snapTimer);
  }

  onMount(() => {
    const read = () => win.isMaximized().then((m) => (maximized = m), () => {});
    read();
    const unlisten = win.onResized(read);
    // A press anywhere else, or Escape, closes an open dropdown.
    const away = (e: PointerEvent) => {
      if (open && !bar.contains(e.target as Node)) open = null;
    };
    const key = (e: KeyboardEvent) => {
      if (open && e.code === "Escape") {
        e.stopPropagation();
        open = null;
      }
    };
    window.addEventListener("pointerdown", away, true);
    window.addEventListener("keydown", key, true);
    return () => {
      clearTimeout(snapTimer);
      unlisten.then((f) => f());
      window.removeEventListener("pointerdown", away, true);
      window.removeEventListener("keydown", key, true);
    };
  });
</script>

<header class="bar" bind:this={bar} style:height="{TITLE}px" data-tauri-drag-region>
  <img class="icon" src={icon} alt="" data-tauri-drag-region />
  {#each MENUS as m (m.id)}
    <div class="menu">
      <button
        class="tool"
        class:on={open === m.id}
        title={m.label}
        aria-label={m.label}
        aria-haspopup="menu"
        aria-expanded={open === m.id}
        disabled={menu.locked}
        onclick={() => (open = open === m.id ? null : m.id)}
        onpointerenter={() => {
          // Like a menu bar: with one open, pointing at the other opens it instead.
          if (open && open !== m.id) open = m.id;
        }}
      >
        {#if m.id === "file"}
          <svg viewBox="0 0 16 16" aria-hidden="true"
            ><path d="M4 1.5h5l3 3v10H4z M9 1.5v3h3" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" /></svg
          >
        {:else}
          <svg viewBox="0 0 16 16" aria-hidden="true"
            ><rect x="1.5" y="2.5" width="13" height="11" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.2" /><path
              d="M10 2.5v11 M1.5 10h8.5"
              stroke="currentColor"
              stroke-width="1.2"
            /></svg
          >
        {/if}
        <span>{m.label}</span>
      </button>
      {#if open === m.id}
        <div class="drop" role="menu">
          {#each m.items as item (item.id)}
            {#if item.separator}<div class="sep" role="separator"></div>{/if}
            <button
              role={item.check ? "menuitemcheckbox" : "menuitem"}
              aria-checked={item.check ? checked(item, menu) : undefined}
              disabled={!enabled(item, menu)}
              onclick={() => choose(item.id)}
            >
              <span class="mark">{item.check && checked(item, menu) ? "✓" : ""}</span>
              <span class="label">{item.label}</span>
              <span class="keys">{item.keys ?? ""}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {/each}
  <button
    class="tool"
    title="Settings (Ctrl+,)"
    aria-label="Settings"
    disabled={menu.locked}
    onclick={() => choose("settings")}
  >
    <svg viewBox="0 0 16 16" aria-hidden="true"
      ><circle cx="8" cy="8" r="2.2" fill="none" stroke="currentColor" stroke-width="1.2" /><path
        d="M8 1.5v2 M8 12.5v2 M1.5 8h2 M12.5 8h2 M3.4 3.4l1.4 1.4 M11.2 11.2l1.4 1.4 M3.4 12.6l1.4-1.4 M11.2 4.8l1.4-1.4"
        stroke="currentColor"
        stroke-width="1.2"
        stroke-linecap="round"
      /></svg
    >
  </button>
  <div class="title" data-tauri-drag-region>Junctur</div>
  <div class="controls">
    <button class="win" aria-label="Minimise" onclick={() => void win.minimize()}>
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5h10" stroke="currentColor" /></svg>
    </button>
    <button
      class="win"
      aria-label={maximized ? "Restore" : "Maximise"}
      onclick={() => {
        leave();
        void win.toggleMaximize();
      }}
      onpointerenter={rest}
      onpointerleave={leave}
    >
      {#if maximized}
        <svg viewBox="0 0 10 10" aria-hidden="true"
          ><path d="M2.5 2.5V.5h7v7h-2 M.5 2.5h7v7h-7z" fill="none" stroke="currentColor" /></svg
        >
      {:else}
        <svg viewBox="0 0 10 10" aria-hidden="true"><rect x=".5" y=".5" width="9" height="9" fill="none" stroke="currentColor" /></svg>
      {/if}
    </button>
    <button class="win close" aria-label="Close" onclick={() => void win.close()}>
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10 M10 0L0 10" stroke="currentColor" /></svg>
    </button>
  </div>
</header>

<style>
  .bar {
    position: relative;
    z-index: 30;
    display: flex;
    align-items: center;
    gap: 2px;
    padding-left: 8px;
    background: var(--panel-solid);
    border-bottom: 1px solid var(--line);
    box-sizing: border-box;
  }
  .icon {
    width: 16px;
    height: 16px;
    margin-right: 6px;
  }
  .menu {
    position: relative;
  }
  .tool {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 8px;
    border: none;
    background: none;
    color: var(--muted);
  }
  .tool:hover,
  .tool.on {
    color: var(--text);
  }
  .tool.on {
    background: rgba(255, 255, 255, 0.12);
  }
  .tool svg {
    width: 15px;
    height: 15px;
  }
  .tool span {
    font-size: 13px;
  }
  .title {
    flex: 1;
    align-self: stretch;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
    font-size: 12px;
  }
  .controls {
    display: flex;
    align-self: stretch;
  }
  .win {
    width: 46px;
    border: none;
    border-radius: 0;
    background: none;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text);
  }
  .win svg {
    width: 10px;
    height: 10px;
  }
  .win.close:hover {
    background: #c42b1c;
    color: #fff;
  }
  .drop {
    position: absolute;
    top: 30px;
    left: 0;
    min-width: 240px;
    padding: 4px;
    background: var(--panel-solid);
    border: 1px solid var(--line);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
  }
  .drop button {
    display: grid;
    grid-template-columns: 18px 1fr auto;
    align-items: center;
    width: 100%;
    padding: 6px 8px;
    border: none;
    background: none;
    text-align: left;
  }
  .drop button:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.1);
  }
  .mark {
    color: var(--accent);
  }
  .keys {
    color: var(--muted);
    font-size: 12px;
    padding-left: 24px;
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--line);
  }
</style>
