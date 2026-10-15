<script lang="ts">
  // Junctur's own title bar: the window is frameless. Windows shows snap layouts only for a native
  // maximise button, so resting on this one asks the app for them (src/window.rs).
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
    <!-- A cog: one outline of eight teeth (tips at radius 6.6, roots at 4.9) around a hole, so it
         does not read as a sun. -->
    <svg viewBox="0 0 16 16" aria-hidden="true"
      ><path
        d="M12.73 6.73L14.54 7.08 14.54 8.92 12.73 9.27 12.24 10.45 13.27 11.97 11.97 13.27 10.45 12.24 9.27 12.73 8.92 14.54 7.08 14.54 6.73 12.73 5.55 12.24 4.03 13.27 2.73 11.97 3.76 10.45 3.27 9.27 1.46 8.92 1.46 7.08 3.27 6.73 3.76 5.55 2.73 4.03 4.03 2.73 5.55 3.76 6.73 3.27 7.08 1.46 8.92 1.46 9.27 3.27 10.45 3.76 11.97 2.73 13.27 4.03 12.24 5.55Z"
        fill="none"
        stroke="currentColor"
        stroke-width="1.2"
        stroke-linejoin="round"
      /><circle cx="8" cy="8" r="2.1" fill="none" stroke="currentColor" stroke-width="1.2" /></svg
    >
  </button>
  <!-- The way back to the start window, out here as well as in File so it can be seen. Only while
       there is a recording to go back from. Not a drag region. -->
  {#if menu.closable}
    <button
      class="tool"
      title="Back to recordings (Ctrl+W)"
      aria-label="Back to recordings"
      disabled={menu.locked}
      onclick={() => choose("close")}
    >
      <svg viewBox="0 0 16 16" aria-hidden="true"
        ><path
          d="M13.5 8h-11 M7 3.5L2.5 8 7 12.5"
          fill="none"
          stroke="currentColor"
          stroke-width="1.2"
          stroke-linecap="round"
          stroke-linejoin="round"
        /></svg
      >
    </button>
  {/if}
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
