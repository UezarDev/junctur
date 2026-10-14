<script lang="ts">
  // The Whisper model picker in Settings (2v). It was a native select listing the names the app
  // sends ("large-v3 q5_0"); a native select can show neither an icon nor a second line, so this is
  // a select-only combobox (the WAI-ARIA pattern): a button showing the chosen model, opening a
  // listbox of every model with its family's icon, a readable name and a line saying how fast, how
  // big or where it runs. Names and lines come from `src/settings.rs` (`ModelChoice`); the icons
  // are drawn here, one per family.
  //
  // Focus stays on the button while the list is open (`aria-activedescendant` names the highlighted
  // option), as a native select does, so Tab leaves from where it entered. Keys: Down, Up, Enter,
  // Space or Alt+Down open it; in it Up/Down/Home/End move, a letter jumps to the next name starting
  // with it, Enter or Space picks and closes, Tab picks and moves on (as the pattern says), Escape
  // closes the list and never the dialog round it. The list lives in `<body>` and takes the shared
  // popup slot (floating.svelte.ts): the dialog scrolls and would clip it, and opening it closes any
  // (i) tip, as an (i) opening closes it.
  import type { ModelChoice } from "./api";
  import { place, portal, release, slot } from "./floating.svelte";

  let {
    id,
    models,
    value,
    onChange,
  }: { id: string; models: ModelChoice[]; value: string; onChange: (key: string) => void } = $props();

  const listId = $derived(`${id}-list`);
  const slotId = $derived(`${id}-picker`);
  /** The list is at least this wide, so a summary fits on its line beside a narrow trigger. */
  const MIN_WIDTH = 300;

  let button = $state<HTMLButtonElement>();
  let list = $state<HTMLDivElement>();
  let pos = $state<{ top: number; left: number; width: number } | null>(null);
  /** The highlighted option while open (an index into `models`). */
  let active = $state(0);
  const open = $derived(slot.open === slotId);
  const chosen = $derived(models.find((m) => m.key === value) ?? models[0]);

  function show() {
    active = Math.max(
      0,
      models.findIndex((m) => m.key === value),
    );
    slot.open = slotId;
  }
  function hide() {
    release(slotId);
  }
  function pick(i: number) {
    const m = models[i];
    if (m && m.key !== value) onChange(m.key);
    hide();
  }

  /** The next model after the highlighted one whose name starts with `letter`, wrapping. */
  function jump(letter: string) {
    const l = letter.toLowerCase();
    for (let n = 1; n <= models.length; n++) {
      const i = (active + n) % models.length;
      if (models[i].name.toLowerCase().startsWith(l)) return (active = i);
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", "Space"].includes(e.code)) {
        e.preventDefault();
        show();
      }
      return;
    }
    switch (e.code) {
      case "ArrowDown":
        if (e.altKey) break;
        active = Math.min(models.length - 1, active + 1);
        break;
      case "ArrowUp":
        // Alt+Up closes on the highlighted model, as a native select does.
        if (e.altKey) pick(active);
        else active = Math.max(0, active - 1);
        break;
      case "Home":
        active = 0;
        break;
      case "End":
        active = models.length - 1;
        break;
      case "Enter":
      case "Space":
        pick(active);
        break;
      case "Tab":
        // Picks and lets the Tab move focus on.
        pick(active);
        return;
      case "Escape":
        // Closes the list only: the Settings dialog listens on the window for Escape.
        e.stopPropagation();
        hide();
        break;
      default:
        if (e.key.length === 1 && /\p{L}/u.test(e.key)) jump(e.key);
        else return;
    }
    e.preventDefault();
  }

  function measure() {
    if (!list || !button) return;
    const r = button.getBoundingClientRect();
    const width = Math.max(r.width, MIN_WIDTH);
    pos = {
      ...place(r, width, list.offsetHeight, { width: window.innerWidth, height: window.innerHeight }, "below"),
      width,
    };
  }

  // Placed against its real height once drawn; hidden until then so it never flashes at 0,0.
  $effect(() => {
    if (!open || !list || !button) {
      pos = null;
      return;
    }
    measure();
  });

  // While open: a press outside closes it (the trigger toggles it itself), as do a scroll of
  // anything round the trigger (a native select's list closes then too) and a resize.
  $effect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => {
      const t = e.target as Node | null;
      if (t && !button?.contains(t) && !list?.contains(t)) hide();
    };
    const scrolled = (e: Event) => {
      if (!(e.target instanceof Node && list?.contains(e.target))) hide();
    };
    window.addEventListener("pointerdown", down, true);
    window.addEventListener("scroll", scrolled, true);
    window.addEventListener("resize", hide);
    return () => {
      window.removeEventListener("pointerdown", down, true);
      window.removeEventListener("scroll", scrolled, true);
      window.removeEventListener("resize", hide);
    };
  });

  // Unmounted while open (the dialog closed), the slot would keep an id nothing releases.
  $effect(() => () => hide());
</script>

{#snippet icon(family: string)}
  <span class="icon {family}" aria-hidden="true">
    <svg viewBox="0 0 16 16" focusable="false">
      {#if family === "turbo"}
        <!-- A bolt: the fast one. -->
        <path d="M9.2 1 3.5 9h3.6L6.4 15l6.1-8.4H8.9L9.2 1Z" />
      {:else if family === "large"}
        <!-- A cut gem: the best one. -->
        <path d="M4.3 2h7.4L15 6.2 8 14.5 1 6.2 4.3 2Zm.6 1.4L3 5.7h2.7l.9-2.3H4.9Zm2.9 0L6.9 5.7h2.2l-.9-2.3h-.4Zm1.7 0 .9 2.3H13l-1.9-2.3H9.5ZM3.2 7.1 7 11.6 5.6 7.1H3.2Zm3.9 0L8 10.3l.9-3.2H7.1Zm3.3 0L9 11.6l3.8-4.5h-2.4Z" fill-rule="evenodd" />
      {:else}
        <!-- A spark: chosen for you. -->
        <path d="M8 1.5c.4 2.9 1.6 4.1 4.5 4.5-2.9.4-4.1 1.6-4.5 4.5-.4-2.9-1.6-4.1-4.5-4.5 2.9-.4 4.1-1.6 4.5-4.5Zm4.3 8.2c.2 1.4.8 2 2.2 2.2-1.4.2-2 .8-2.2 2.2-.2-1.4-.8-2-2.2-2.2 1.4-.2 2-.8 2.2-2.2Z" />
      {/if}
    </svg>
  </span>
{/snippet}

<button
  type="button"
  {id}
  class="picker"
  bind:this={button}
  role="combobox"
  aria-haspopup="listbox"
  aria-expanded={open}
  aria-controls={listId}
  aria-activedescendant={open ? `${listId}-${active}` : undefined}
  onclick={() => (open ? hide() : show())}
  {onkeydown}
  onblur={() => open && hide()}
>
  {#if chosen}{@render icon(chosen.family)}<span class="chosen">{chosen.name}</span>{/if}
  <svg class="chevron" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
    <path d="M4 6l4 4 4-4" />
  </svg>
</button>
{#if open}
  <!-- Pointer presses are kept from taking focus, so it stays on the button (and its blur does not
       close the list before the click lands). -->
  <div
    use:portal
    bind:this={list}
    id={listId}
    role="listbox"
    tabindex="-1"
    aria-label="Whisper models"
    class="list"
    style:top="{pos?.top ?? 0}px"
    style:left="{pos?.left ?? 0}px"
    style:width="{pos?.width ?? MIN_WIDTH}px"
    style:visibility={pos ? null : "hidden"}
    onpointerdown={(e) => e.preventDefault()}
  >
    <!-- The keys are the button's (focus never leaves it); an option's empty onkeydown is for the
         a11y check. -->
    {#each models as m, i (m.key)}
      <div
        id="{listId}-{i}"
        role="option"
        tabindex="-1"
        aria-selected={m.key === value}
        class="option"
        class:active={i === active}
        onpointermove={() => (active = i)}
        onclick={() => pick(i)}
        onkeydown={() => {}}
      >
        {@render icon(m.family)}
        <span class="text">
          <span class="name">{m.name}</span>
          <span class="summary">{m.summary}</span>
        </span>
        {#if m.key === value}
          <svg class="tick" viewBox="0 0 16 16" aria-hidden="true" focusable="false"><path d="M3.5 8.5l3 3 6-7" /></svg>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .picker {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
    width: var(--control-width, 200px);
    min-width: 0;
    padding: 4px 8px 4px 5px;
    text-align: left;
  }
  .picker:focus-visible {
    outline: 1px solid var(--accent);
    outline-offset: 0;
  }
  .picker[aria-expanded="true"] {
    border-color: var(--accent);
  }
  .chosen {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chevron,
  .tick {
    flex: none;
    width: 14px;
    height: 14px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .chevron {
    color: var(--muted);
  }
  .tick {
    color: var(--accent);
  }
  /* A tinted tile per family, so the three read apart at a glance. */
  .icon {
    display: inline-flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 5px;
    background: color-mix(in srgb, var(--tint) 18%, transparent);
    color: var(--tint);
  }
  .icon svg {
    width: 13px;
    height: 13px;
    fill: currentColor;
  }
  .icon.auto {
    --tint: var(--accent);
  }
  .icon.turbo {
    --tint: #6cb6ff;
  }
  .icon.large {
    --tint: #c39bff;
  }
  /* Opaque: the page under the UI is mpv's picture. Over the Settings backdrop (z-index 40). */
  .list {
    position: fixed;
    z-index: 100;
    box-sizing: border-box;
    padding: 4px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: 8px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.55);
    outline: none;
  }
  .option {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  .option .icon {
    width: 28px;
    height: 28px;
    border-radius: 7px;
  }
  .option .icon svg {
    width: 16px;
    height: 16px;
  }
  .option.active {
    background: rgba(255, 255, 255, 0.08);
  }
  .text {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }
  .name {
    font-weight: 600;
  }
  .summary {
    color: var(--muted);
    font-size: 12px;
  }
</style>
