<script lang="ts">
  // The caption settings' fields (2i): moved out of the Captions panel so Settings edits the
  // defaults with the very same fields. Every field of `Captions` is here once, in parts the panel
  // places between its own sections (`switches`: export and preview, 5d; `style`: bold and italic,
  // 5d, underline, 5f; `look`: font, size, position, outline, 5g; `overlap`: layout and
  // protagonist, 5g; `resolve`: the Text+ template, 5g; `cut`: how cues are cut, 5a, and the word
  // limit, 5i; `text`: case, punctuation, censoring and replacements, 5j, in `CaptionText`)
  // and Settings places under the same headings (2t; `all` shows every part at once); a field
  // added to `Captions` goes into one part and shows in both. Each is a `Field`, its explanation
  // behind an (i) rather than in a `title` (2t). Number fields commit on change, inside the engine's limits (core
  // edit/captions.rs); anything else snaps back to `value`. `names` (the recording's
  // speakers, in the panel) are offered as protagonists; any name can be
  // typed, so Settings' default can name a remembered voice.
  import type { CaptionLayout, Captions } from "./api";
  import CaptionText from "./CaptionText.svelte";
  import Field from "./Field.svelte";

  type Part = "switches" | "style" | "look" | "overlap" | "resolve" | "cut" | "text" | "all";

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
  } as const;

  const WHOLE = new Set(["chars", "lines", "words", "outlineWidth"]);

  const LAYOUTS: { key: CaptionLayout; label: string }[] = [
    { key: "stacked", label: "Stacked" },
    { key: "side", label: "Side by side" },
    { key: "topBottom", label: "Top and bottom" },
  ];

  type Flag = "on" | "preview" | "bold" | "italic" | "underline" | "outline";
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

  /** A name field: trimmed, quotes refused (the edit list writes these in quotes). */
  function text(e: Event, key: "font" | "protagonist" | "template", needed: boolean) {
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
{#if shows("resolve")}
  <Field
    label="Resolve template"
    info="A Text+ title in Resolve's media pool to copy for every caption; empty for Junctur's own. Yours keeps its shadow and background."
  >
    {#snippet children(id)}
      <input {id} placeholder="Junctur's Text+" value={value.template} {disabled} onchange={(e) => text(e, "template", false)} />
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
</style>
