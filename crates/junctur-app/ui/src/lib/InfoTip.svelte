<script lang="ts">
  // A setting's explanation behind an (i) button: a real button so Tab and focus open it too. A
  // scroll closes it only once the trigger is out of sight, so a Tab that scrolls to it keeps it.
  import { place, portal, release, slot } from "./floating.svelte";

  let { label, text }: { label: string; text: string } = $props();

  const id = $props.id();
  const WIDTH = 260;
  /** Long enough to cross the gap into the popup, short enough not to linger after a glance. */
  const CLOSE_MS = 140;

  let button = $state<HTMLButtonElement>();
  let popup = $state<HTMLDivElement>();
  let pos = $state<{ top: number; left: number } | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const open = $derived(slot.open === id);

  function show() {
    clearTimeout(timer);
    slot.open = id;
  }
  function hideSoon() {
    clearTimeout(timer);
    timer = setTimeout(() => release(id), CLOSE_MS);
  }
  function hide() {
    clearTimeout(timer);
    release(id);
  }

  function measure() {
    if (!popup || !button) return;
    pos = place(button.getBoundingClientRect(), WIDTH, popup.offsetHeight, {
      width: window.innerWidth,
      height: window.innerHeight,
    });
  }

  // Placed against its real height once drawn; hidden until then so it never flashes at 0,0.
  $effect(() => {
    void text;
    if (!open || !popup || !button) {
      pos = null;
      return;
    }
    measure();
  });

  /** A scroll of the box holding the trigger (or of the page) moves the popup, or closes it once
   * the trigger is scrolled out of that box or the window. Other boxes' scrolls leave it alone. */
  function scrolled(e: Event) {
    if (!button) return;
    const box = e.target instanceof Element ? e.target : null;
    if (box && !box.contains(button)) return;
    const r = button.getBoundingClientRect();
    const b = box?.getBoundingClientRect();
    const top = Math.max(b?.top ?? 0, 0);
    const bottom = Math.min(b?.bottom ?? window.innerHeight, window.innerHeight);
    const left = Math.max(b?.left ?? 0, 0);
    const right = Math.min(b?.right ?? window.innerWidth, window.innerWidth);
    if (r.bottom > top && r.top < bottom && r.right > left && r.left < right) measure();
    else hide();
  }

  $effect(() => {
    if (!open) return;
    const close = () => hide();
    const key = (e: KeyboardEvent) => e.code === "Escape" && hide();
    window.addEventListener("scroll", scrolled, true);
    window.addEventListener("resize", close);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("scroll", scrolled, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("keydown", key);
    };
  });

  // Unmounted while open (the dialog closed, the section went), the slot would keep an id nothing
  // releases.
  $effect(() => () => hide());
</script>

<button
  type="button"
  class="info"
  bind:this={button}
  aria-label="About {label}"
  aria-describedby={open ? id : undefined}
  onmouseenter={show}
  onmouseleave={hideSoon}
  onfocus={show}
  onblur={hide}
  onclick={show}
  onkeydown={(e) => {
    // Escape on the (i) closes the tip, not the Settings dialog round it.
    if (e.code === "Escape" && open) {
      e.stopPropagation();
      hide();
    }
  }}
>
  <!-- One evenodd path, so the "i" is a hole in the disc and takes the button's colour. -->
  <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
    <path
      fill-rule="evenodd"
      d="M8 0a8 8 0 1 0 0 16A8 8 0 0 0 8 0Z M8 3.75a1.15 1.15 0 1 0 0 2.3 1.15 1.15 0 0 0 0-2.3Z M8 7.3a1 1 0 0 0-1 1v3.6a1 1 0 0 0 2 0V8.3a1 1 0 0 0-1-1Z"
    />
  </svg>
</button>
{#if open}
  <div
    use:portal
    bind:this={popup}
    {id}
    role="tooltip"
    class="tip"
    style:top="{pos?.top ?? 0}px"
    style:left="{pos?.left ?? 0}px"
    style:width="{WIDTH}px"
    style:visibility={pos ? null : "hidden"}
    onmouseenter={() => clearTimeout(timer)}
    onmouseleave={hideSoon}
  >
    {text}
  </div>
{/if}

<style>
  .info {
    display: inline-flex;
    flex-shrink: 0;
    width: 14px;
    height: 14px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: none;
    color: var(--muted);
    vertical-align: middle;
    cursor: help;
  }
  .info:hover,
  .info:focus-visible {
    background: none;
    color: var(--accent);
  }
  .info:focus-visible {
    outline: 1px solid var(--accent);
    outline-offset: 2px;
  }
  svg {
    width: 100%;
    height: 100%;
    fill: currentColor;
  }
  /* Opaque: the page under the UI is mpv's picture. */
  .tip {
    position: fixed;
    z-index: 100;
    box-sizing: border-box;
    padding: 8px 10px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: 6px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.55);
    color: var(--muted);
    font-size: 12px;
    font-weight: 400;
    line-height: 1.5;
    letter-spacing: normal;
    text-transform: none;
    white-space: normal;
    user-select: text;
  }
</style>
