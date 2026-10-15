<script lang="ts">
  // The question when Resolve already has the timeline a send makes. Its own dialog because the
  // native one has three buttons and closing it answers as the third. Modal: a capture listener
  // stops every key from reaching the page, except Ctrl+C.
  import { onMount } from "svelte";
  import type { TimelineAnswer } from "./api";

  let { name, onPick }: { name: string; onPick: (answer: TimelineAnswer | null) => void } = $props();

  let box: HTMLElement;
  let picked = false;
  function pick(a: TimelineAnswer | null) {
    if (picked) return;
    picked = true;
    onPick(a);
  }

  const buttons = () => Array.from(box.querySelectorAll("button"));

  onMount(() => {
    buttons()[0]?.focus();
  });

  function key(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.code === "Escape") {
      e.preventDefault();
      pick(null);
    } else if (e.code === "Tab") {
      e.preventDefault();
      const all = buttons();
      const at = all.indexOf(document.activeElement as HTMLButtonElement);
      const next = at < 0 ? 0 : (at + (e.shiftKey ? all.length - 1 : 1)) % all.length;
      all[next]?.focus();
    } else if (e.code === "Enter" || e.code === "NumpadEnter" || e.code === "Space") {
      e.preventDefault();
      const b = document.activeElement;
      if (b instanceof HTMLButtonElement && box.contains(b)) b.click();
    } else if (!((e.ctrlKey || e.metaKey) && e.code === "KeyC")) {
      // Nothing under the dialog acts on a key while it is open (Space would play, Delete cut).
      e.preventDefault();
    }
  }
</script>

<svelte:window onkeydowncapture={key} />

<div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && pick(null)}>
  <div class="dialog" bind:this={box} role="alertdialog" aria-modal="true" aria-labelledby="exists-title" aria-describedby="exists-text">
    <h2 id="exists-title">This timeline is already in Resolve</h2>
    <div id="exists-text">
      <p>Resolve already has a timeline called "{name}".</p>
      <dl>
        <dt>Create new timeline</dt>
        <dd>Makes "{name} 2" (or the next free number) beside it and leaves it as it is.</dd>
        <dt>Update captions only</dt>
        <dd>Refreshes only Junctur's captions on "{name}" and leaves its cuts and your Resolve changes alone.</dd>
        <dt>Replace</dt>
        <dd>Deletes "{name}" and makes it again from this edit.</dd>
      </dl>
    </div>
    <div class="actions">
      <button class="primary" onclick={() => pick("new")}>Create new timeline (recommended)</button>
      <button onclick={() => pick("update")}>Update captions only</button>
      <button onclick={() => pick("replace")}>Replace</button>
      <button onclick={() => pick(null)}>Cancel</button>
    </div>
  </div>
</div>

<style>
  /* Over the whole window, the title bar (z-index 30) too, as a native dialog would be. */
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 45;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.55);
  }
  .dialog {
    width: min(480px, calc(100% - 32px));
    max-height: calc(100% - 32px);
    overflow-y: auto;
    padding: 18px 22px;
    background: var(--panel-solid);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.6);
    box-sizing: border-box;
    user-select: text;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 16px;
  }
  p {
    margin: 0 0 8px;
  }
  dl {
    margin: 0;
  }
  dt {
    font-weight: 600;
    margin-top: 8px;
  }
  dd {
    margin: 2px 0 0;
    color: var(--muted);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
