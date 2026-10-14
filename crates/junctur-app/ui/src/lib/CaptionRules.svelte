<script lang="ts">
  // The caption settings' fields, shared so Settings edits the defaults with the very same fields.
  // Every `Captions` field is in one part, which the panel and Settings each place under a heading.
  import { resolveTemplates, type CaptionAnimation, type CaptionLayout, type Captions } from "./api";
  import CaptionText from "./CaptionText.svelte";
  import Field from "./Field.svelte";

  type Part = "switches" | "style" | "look" | "overlap" | "motion" | "resolve" | "cut" | "text" | "all";

  let {
    value,
    part = "all",
    disabled = false,
    names = [],
    onChange,
    onError,
  }: {
    value: Captions;
    part?: Part;
    disabled?: boolean;
    names?: string[];
    onChange: (c: Captions) => void;
    onError: (message: string) => void;
  } = $props();

  // One datalist per instance: the panel shows several parts at once.
  const listId = `protagonists-${Math.random().toString(36).slice(2)}`;

  const shows = (p: Part) => part === "all" || part === p;

  const LIMITS = {
    chars: { min: 10, max: 80, name: "Characters per line" },
    lines: { min: 1, max: 3, name: "Lines per speaker" },
    words: { min: 0, max: 20, name: "Words per caption" },
    seconds: { min: 1, max: 15, name: "Longest caption" },
    size: { min: 2, max: 15, name: "Size" },
    position: { min: 0, max: 45, name: "Height from the bottom" },
    outlineWidth: { min: 1, max: 10, name: "Outline thickness" },
    animationLength: { min: 0.05, max: 1, name: "Animation length" },
  } as const;

  const WHOLE = new Set(["chars", "lines", "words", "outlineWidth"]);

  const LAYOUTS: { key: CaptionLayout; label: string }[] = [
    { key: "stacked", label: "Stacked" },
    { key: "side", label: "Side by side" },
    { key: "topBottom", label: "Top and bottom" },
  ];

  const ANIMATIONS: { key: CaptionAnimation; label: string }[] = [
    { key: "none", label: "None" },
    { key: "fade", label: "Fade" },
    { key: "pop", label: "Pop in" },
    { key: "slide", label: "Slide up" },
  ];

  type Flag = "on" | "preview" | "bold" | "italic" | "underline" | "outline" | "highlight";
  const SWITCHES: { key: Flag; label: string; info: string }[] = [
    { key: "on", label: "Export captions", info: "Rendering also writes the captions (.srt) beside the video." },
    { key: "preview", label: "Preview captions", info: "Shows the captions over the video as it plays, cut as the export cuts them." },
  ];
  const STYLE: { key: Flag; label: string }[] = [
    { key: "bold", label: "Bold" },
    { key: "italic", label: "Italic" },
    { key: "underline", label: "Underline" },
  ];

  function flag(e: Event, key: Flag) {
    onChange({ ...value, [key]: (e.currentTarget as HTMLInputElement).checked });
  }

  function number(e: Event, key: keyof typeof LIMITS) {
    const el = e.currentTarget as HTMLInputElement;
    const v = Number(el.value);
    const { min, max, name } = LIMITS[key];
    if (!Number.isFinite(v) || v < min || v > max) {
      el.value = String(value[key]);
      onError(`${name} must be ${min} to ${max}`);
      return;
    }
    onChange({ ...value, [key]: WHOLE.has(key) ? Math.round(v) : v });
  }

  // The template picker: the Fusion Titles in the open Resolve project's media pool, asked only when
  // the list is opened or refreshed, since every ask waits on Resolve. `null` until asked.
  let titles = $state<string[] | null>(null);
  let fetching = $state(false);
  let fetchError = $state<string | null>(null);
  async function fetchTitles() {
    if (fetching) return;
    fetching = true;
    fetchError = null;
    try {
      titles = await resolveTemplates();
    } catch (e) {
      fetchError = String(e);
    } finally {
      fetching = false;
    }
  }
  // The picked template is always the second entry, so the selected option stays the same element
  // when the list arrives; marked when Resolve's list came back without it.
  const others = $derived((titles ?? []).filter((t) => t !== value.template));
  const missing = $derived(titles !== null && !!value.template && !titles.includes(value.template));

  /** A name field: trimmed, quotes refused (the edit list writes these in quotes). */
  function text(e: Event, key: "font" | "protagonist", needed: boolean) {
    const el = e.currentTarget as HTMLInputElement;
    const v = el.value.trim();
    if (v.includes('"') || (needed && !v)) {
      el.value = value[key];
      onError(needed && !v ? "A font is needed" : "Names cannot hold quotes");
      return;
    }
    if (v !== value[key]) onChange({ ...value, [key]: v });
  }
</script>

{#snippet check(key: Flag, label: string, info?: string)}
  <Field kind="check" {label} {info}>
    {#snippet children(id)}
      <input {id} type="checkbox" checked={value[key]} {disabled} onchange={(e) => flag(e, key)} />
    {/snippet}
  </Field>
{/snippet}

{#snippet num(key: keyof typeof LIMITS, label: string, step: number, info?: string)}
  <Field {label} {info}>
    {#snippet children(id)}
      <input
        {id}
        type="number"
        min={LIMITS[key].min}
        max={LIMITS[key].max}
        {step}
        value={value[key]}
        {disabled}
        onchange={(e) => number(e, key)}
      />
    {/snippet}
  </Field>
{/snippet}

{#if shows("switches")}
  {#each SWITCHES as s (s.key)}{@render check(s.key, s.label, s.info)}{/each}
{/if}
{#if shows("style")}
  <div class="inline">{#each STYLE as s (s.key)}{@render check(s.key, s.label)}{/each}</div>
{/if}
{#if shows("look")}
  <Field label="Font">
    {#snippet children(id)}
      <input {id} value={value.font} {disabled} onchange={(e) => text(e, "font", true)} />
    {/snippet}
  </Field>
  {@render num("size", "Size (%)", 0.5, "Line height, as a share of the picture's height.")}
  {@render num("position", "Height from the bottom (%)", 1, "Where the captions' bottom edge sits, as a share of the picture's height.")}
  {@render check("outline", "Outline", "An outline round the letters, in the preview and in Resolve's Text+.")}
  {#if value.outline}
    {@render num("outlineWidth", LIMITS.outlineWidth.name, 1)}
    <Field label="Outline colour">
      {#snippet children(id)}
        <input
          {id}
          type="color"
          value={value.outlineColor}
          {disabled}
          onchange={(e) => onChange({ ...value, outlineColor: (e.currentTarget as HTMLInputElement).value.toUpperCase() })}
        />
      {/snippet}
    </Field>
  {/if}
{/if}
{#if shows("overlap")}
  <Field label="Layout" info="Stacked puts the later speaker above; top and bottom puts one at the bottom, one at the top.">
    {#snippet children(id)}
      <select
        {id}
        value={value.layout}
        {disabled}
        onchange={(e) => onChange({ ...value, layout: (e.currentTarget as HTMLSelectElement).value as CaptionLayout })}
      >
        {#each LAYOUTS as l (l.key)}<option value={l.key}>{l.label}</option>{/each}
      </select>
    {/snippet}
  </Field>
  <Field label="Protagonist" info="This speaker's caption keeps the normal spot; the others move round it.">
    {#snippet children(id)}
      <input {id} list={listId} placeholder="None" value={value.protagonist} {disabled} onchange={(e) => text(e, "protagonist", false)} />
    {/snippet}
  </Field>
  <datalist id={listId}>{#each names as n (n)}<option value={n}></option>{/each}</datalist>
{/if}
{#if shows("motion")}
  {@render check(
    "highlight",
    "Highlight the word being said",
    "In Resolve and the preview, the word being said lights up, word by word.",
  )}
  {#if value.highlight}
    <!-- Empty means each speaker's own colour, which no single swatch can show: then the field
         says so in words and offers a colour to pick instead (yellow, the colourless speaker's). -->
    <Field label="Highlight colour" info="Each speaker's own colour lights the word on white text, unless one colour is picked.">
      {#snippet children(id)}
        <span class="pair">
          {#if value.highlightColor}
            <input
              {id}
              type="color"
              value={value.highlightColor}
              {disabled}
              onchange={(e) =>
                onChange({ ...value, highlightColor: (e.currentTarget as HTMLInputElement).value.toUpperCase() })}
            />
            <button
              class="small"
              {disabled}
              title="Light the word in each speaker's own colour"
              onclick={() => onChange({ ...value, highlightColor: "" })}>Speaker's</button
            >
          {:else}
            <span class="muted">Each speaker's</span>
            <button
              {id}
              class="small"
              {disabled}
              title="Light every speaker's words in one colour you pick"
              onclick={() => onChange({ ...value, highlightColor: "#FFD400" })}>Pick one</button
            >
          {/if}
        </span>
      {/snippet}
    </Field>
  {/if}
  <Field label="Animation" info="How each caption comes in and goes out, in Resolve and the preview.">
    {#snippet children(id)}
      <select
        {id}
        value={value.animation}
        {disabled}
        onchange={(e) =>
          onChange({ ...value, animation: (e.currentTarget as HTMLSelectElement).value as CaptionAnimation })}
      >
        {#each ANIMATIONS as a (a.key)}<option value={a.key}>{a.label}</option>{/each}
      </select>
    {/snippet}
  </Field>
  {#if value.animation !== "none"}{@render num("animationLength", "Animation length (s)", 0.05)}{/if}
{/if}
{#if shows("resolve")}
  <Field
    label="Resolve template"
    info="A Text+ title in the open Resolve project's media pool to copy for every caption, or Junctur's own. Refresh asks Resolve again. The word highlight and the animation need Junctur's own template (or a copy of its animated caption): a plain Text+ of yours is placed without them."
  >
    {#snippet children(id)}
      <span class="pair">
        <select
          {id}
          value={value.template}
          {disabled}
          onfocus={() => titles === null && fetchTitles()}
          onchange={(e) => onChange({ ...value, template: (e.currentTarget as HTMLSelectElement).value })}
        >
          <option value="">Junctur's own</option>
          {#if value.template}
            <option value={value.template}>{value.template}{missing ? " (not found in Resolve)" : ""}</option>
          {/if}
          {#each others as t (t)}<option value={t}>{t}</option>{/each}
        </select>
        <button
          class="small"
          disabled={disabled || fetching}
          title="Ask Resolve again for the Text+ titles in the open project's media pool"
          onclick={(e) => {
            e.preventDefault();
            fetchTitles();
          }}>{fetching ? "…" : "Refresh"}</button
        >
      </span>
    {/snippet}
  </Field>
  {#if fetchError}<p class="status bad">{fetchError}</p>{/if}
  {#if titles !== null && titles.length === 0 && !fetchError}
    <p class="status">
      No Text+ titles in this Resolve project's media pool: drag a styled Text+ into it, then Refresh.
    </p>
  {/if}
  <Field
    kind="check"
    label="Keep the template's look"
    info="Junctur sets only each caption's text, timing and speaker colour; the font, size, position and outline are the template's."
  >
    {#snippet children(id)}
      <input
        {id}
        type="checkbox"
        checked={value.keepLook && !!value.template}
        disabled={disabled || !value.template}
        onchange={(e) => onChange({ ...value, keepLook: (e.currentTarget as HTMLInputElement).checked })}
      />
    {/snippet}
  </Field>
{/if}
{#if shows("cut")}
  {@render num("chars", LIMITS.chars.name, 1)}
  {@render num("lines", LIMITS.lines.name, 1)}
  {@render num(
    "words",
    LIMITS.words.name,
    1,
    "Most words a caption holds, per speaker when people talk at once. 1 shows one word at a time; 0 is no limit.",
  )}
  {@render num("seconds", "Longest caption (s)", 0.5)}
{/if}
{#if shows("text")}<CaptionText {value} {disabled} {onChange} {onError} />{/if}

<style>
  .inline {
    display: flex;
    flex-wrap: wrap;
    gap: 0 20px;
  }
  .pair {
    display: flex;
    flex: none;
    gap: 6px;
    align-items: center;
  }
  .pair select {
    width: calc(var(--control-width, 200px) - 70px);
    min-width: 0;
  }
  .pair input[type="color"] {
    width: 40px;
    height: 24px;
    padding: 0;
    border: 1px solid var(--line);
    background: none;
  }
  .small {
    font-size: 11px;
    padding: 2px 6px;
  }
  .muted {
    color: var(--muted);
  }
</style>
