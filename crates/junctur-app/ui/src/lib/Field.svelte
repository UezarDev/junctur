<script lang="ts">
  // One setting (2t): its label, its (i) when it needs explaining, and its control, in one of three
  // shapes: `row` (label left, control right in a fixed column so controls line up down a group),
  // `check` (a checkbox and its label) and `stack` (label above a full-width box, for the lists).
  //
  // The label names the control by id, which the control snippet is handed, rather than wrapping
  // it: a <label> round a row that holds the (i) button would label the button (the first labelable
  // element inside it), and a click on the words would press the (i). The control's look is set
  // here (`:global`, since the snippet is the caller's markup) so every field is styled once.
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
    width: 160px;
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
