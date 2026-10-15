<script lang="ts">
  // One setting: label, optional (i) and control, as `row`, `check` or `stack`. The label names the
  // control by id rather than wrapping it, or a click on the words would press the (i) button.
  import type { Snippet } from "svelte";
  import InfoTip from "./InfoTip.svelte";

  let {
    label,
    info,
    kind = "row",
    children,
  }: { label: string; info?: string; kind?: "row" | "check" | "stack"; children: Snippet<[string]> } = $props();

  const id = $props.id();
</script>

{#if kind === "check"}
  <div class="field check">
    <label>{@render children(id)}{label}</label>
    {#if info}<InfoTip {label} text={info} />{/if}
  </div>
{:else}
  <div class="field {kind}">
    <span class="name">
      <label for={id}>{label}</label>
      {#if info}<InfoTip {label} text={info} />{/if}
    </span>
    {@render children(id)}
  </div>
{/if}

<style>
  .field {
    padding: 4px 0;
  }
  .name {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    min-height: 28px;
  }
  .stack {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 24px;
  }
  .check label {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
  }
  .check :global(input) {
    margin: 0;
  }
  .field :global(input:not([type="checkbox"]):not([type="color"])),
  .field :global(textarea) {
    box-sizing: border-box;
    font: inherit;
    color: inherit;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 4px 8px;
    user-select: text;
  }
  .field :global(input:focus-visible),
  .field :global(textarea:focus-visible),
  .field :global(select:focus-visible) {
    outline: 1px solid var(--accent);
    outline-offset: 0;
  }
  .row > :global(select),
  .row > :global(input:not([type="number"]):not([type="color"])) {
    flex: none;
    width: var(--control-width, 200px);
    min-width: 0;
  }
  .row > :global(input[type="number"]) {
    flex: none;
    width: 72px;
  }
  .row > :global(input[type="color"]) {
    width: 40px;
    height: 24px;
    padding: 0;
    border: 1px solid var(--line);
    background: none;
  }
  .stack > :global(textarea) {
    width: 100%;
    resize: vertical;
  }
</style>
