<script lang="ts">
  // The caption settings' fields (2i): moved out of the Captions panel so Settings edits the
  // defaults with the very same fields. Every field of `Captions` is here once, in parts the panel
  // places between its own sections (`switches`: export and preview, 5d; `style`: bold and italic,
  // 5d, underline, 5f; `look`: font, size, position, outline, 5g; `overlap`: layout and
  // protagonist, 5g; `motion`: the word highlight and the animation, 5l; `resolve`: the Text+
  // template, 5g, picked from the Fusion Titles in Resolve's media pool with its look kept or not,
  // 5m; `cut`: how cues are cut, 5a, and the word
  // limit, 5i; `text`: case, punctuation, censoring and replacements, 5j, in `CaptionText`)
  // and Settings shows together (`all`); a field added to `Captions` goes into one part
  // and shows in both. Number fields commit on change, inside the engine's limits (core
  // edit/captions.rs); anything else snaps back to `value`. `names` (the recording's
  // speakers, in the panel) are offered as protagonists; any name can be
  // typed, so Settings' default can name a remembered voice.
  import { resolveTemplates, type CaptionAnimation, type CaptionLayout, type Captions } from "./api";
  import CaptionText from "./CaptionText.svelte";

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
    { key: "stacked", label: "Stacked, the later one above" },
    { key: "side", label: "Side by side" },
    { key: "topBottom", label: "One at the bottom, one at the top" },
  ];

  const ANIMATIONS: { key: CaptionAnimation; label: string }[] = [
    { key: "none", label: "None" },
    { key: "fade", label: "Fade" },
    { key: "pop", label: "Pop in" },
    { key: "slide", label: "Slide up" },
  ];

  type Flag = "on" | "preview" | "bold" | "italic" | "underline" | "outline" | "highlight";
  const SWITCHES: { key: Flag; label: string; title: string }[] = [
    { key: "on", label: "Export captions", title: "On: rendering also writes the captions (.srt) beside the video" },
    { key: "preview", label: "Preview captions", title: "Shows the captions over the video as it plays, as the export cuts them" },
  ];
  const STYLE: { key: Flag; label: string; title: string }[] = [
    { key: "bold", label: "Bold", title: "Every caption bold, in the SRT and in Resolve's Text+" },
    { key: "italic", label: "Italic", title: "Every caption italic, in the SRT and in Resolve's Text+" },
    { key: "underline", label: "Underline", title: "Every caption underlined, in the SRT and in Resolve's Text+" },
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

  // The template picker (5m): the Fusion Titles in the open Resolve project's media pool, asked
  // of the Junctur script in Resolve the first time the list is opened and again on Refresh; never
  // on its own, since every ask waits on Resolve (seconds when the script is not running). Its
  // failure shows under the list rather than as the app's error: opening the list with Resolve
  // closed is ordinary. `null` until asked.
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
  // when the list arrives; marked when Resolve's list came back without it (a name typed before
  // 5m, a default from Settings, a title in another project).
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

{#snippet checks(list: { key: Flag; label: string; title: string }[])}
  {#each list as s (s.key)}
    <label class="check" title={s.title}>
      <input type="checkbox" checked={value[s.key]} {disabled} onchange={(e) => flag(e, s.key)} />
      {s.label}
    </label>
  {/each}
{/snippet}

{#snippet num(key: "size" | "position" | "outlineWidth" | "animationLength", step: number, unit: string)}
  <label class="field">
    <span>{LIMITS[key].name}{unit}</span>
    <input
      type="number"
      min={LIMITS[key].min}
      max={LIMITS[key].max}
      {step}
      value={value[key]}
      {disabled}
      onchange={(e) => number(e, key)}
    />
  </label>
{/snippet}

{#if shows("switches")}{@render checks(SWITCHES)}{/if}
{#if shows("style")}{@render checks(STYLE)}{/if}
{#if shows("look")}
  <label class="field">
    <span>Font</span>
    <input class="name" value={value.font} {disabled} onchange={(e) => text(e, "font", true)} />
  </label>
  {@render num("size", 0.5, " (% of height)")}
  {@render num("position", 1, " (%)")}
  <label class="check" title="An outline round the letters of the Text+ captions and the preview">
    <input type="checkbox" checked={value.outline} {disabled} onchange={(e) => flag(e, "outline")} />
    Outline
  </label>
  {#if value.outline}
    {@render num("outlineWidth", 1, "")}
    <label class="field">
      <span>Outline colour</span>
      <input
        type="color"
        value={value.outlineColor}
        {disabled}
        onchange={(e) => onChange({ ...value, outlineColor: (e.currentTarget as HTMLInputElement).value.toUpperCase() })}
      />
    </label>
  {/if}
{/if}
{#if shows("overlap")}
  <label class="field">
    <span>People talking at once</span>
    <select
      value={value.layout}
      {disabled}
      onchange={(e) => onChange({ ...value, layout: (e.currentTarget as HTMLSelectElement).value as CaptionLayout })}
    >
      {#each LAYOUTS as l (l.key)}<option value={l.key}>{l.label}</option>{/each}
    </select>
  </label>
  <label class="field" title="This speaker's caption always stays at the normal spot; the others move round it">
    <span>Protagonist</span>
    <input
      class="name"
      list={listId}
      placeholder="None"
      value={value.protagonist}
      {disabled}
      onchange={(e) => text(e, "protagonist", false)}
    />
    <datalist id={listId}>{#each names as n (n)}<option value={n}></option>{/each}</datalist>
  </label>
{/if}
{#if shows("motion")}
  <label class="check" title="In Resolve and the preview, the word being said lights up, word by word">
    <input type="checkbox" checked={value.highlight} {disabled} onchange={(e) => flag(e, "highlight")} />
    Highlight the word being said
  </label>
  {#if value.highlight}
    <!-- Empty means each speaker's own colour, which no single swatch can show: then the field
         says so in words and offers a colour to pick instead (yellow, the colourless speaker's). -->
    <div class="field" title="Each speaker's own colour lights the word on white text, unless one colour is picked">
      <span>Highlight colour</span>
      <span class="pair">
        {#if value.highlightColor}
          <input
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
            class="small"
            {disabled}
            title="Light every speaker's words in one colour you pick"
            onclick={() => onChange({ ...value, highlightColor: "#FFD400" })}>Pick one</button
          >
        {/if}
      </span>
    </div>
  {/if}
  <label class="field" title="How each caption comes in and goes out, in Resolve and the preview">
    <span>Animation</span>
    <select
      value={value.animation}
      {disabled}
      onchange={(e) =>
        onChange({ ...value, animation: (e.currentTarget as HTMLSelectElement).value as CaptionAnimation })}
    >
      {#each ANIMATIONS as a (a.key)}<option value={a.key}>{a.label}</option>{/each}
    </select>
  </label>
  {#if value.animation !== "none"}{@render num("animationLength", 0.05, " (s)")}{/if}
{/if}
{#if shows("resolve")}
  <label class="field" title="The Text+ title in Resolve's media pool every caption is a copy of; Junctur's own unless you pick one">
    <span>Resolve template</span>
    <span class="pair">
      <select
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
  </label>
  {#if fetchError}<p class="hint">{fetchError}</p>{/if}
  {#if titles !== null && titles.length === 0 && !fetchError}
    <p class="hint">
      No Text+ titles in this Resolve project's media pool: style a Text+ on a timeline, drag it into the media pool,
      then Refresh.
    </p>
  {/if}
  <label
    class="check"
    title="Junctur sets only each caption's text, timing and speaker colour; the font, size, position and outline are the template's"
  >
    <input
      type="checkbox"
      checked={value.keepLook}
      disabled={disabled || !value.template}
      onchange={(e) => onChange({ ...value, keepLook: (e.currentTarget as HTMLInputElement).checked })}
    />
    Keep the template's look
  </label>
  {#if value.template && (value.highlight || value.animation !== "none")}
    <p class="hint">
      The word highlight and the animation need Junctur's own template (or a copy of its animated caption): a plain
      Text+ of yours is placed without them.
    </p>
  {/if}
{/if}
{#if shows("cut")}
  <label class="field">
    <span>{LIMITS.chars.name}</span>
    <input type="number" min="10" max="80" value={value.chars} {disabled} onchange={(e) => number(e, "chars")} />
  </label>
  <label class="field">
    <span>{LIMITS.lines.name}</span>
    <input type="number" min="1" max="3" value={value.lines} {disabled} onchange={(e) => number(e, "lines")} />
  </label>
  <label
    class="field"
    title="Most words a caption holds, for each speaker when people talk at once; 1 shows one word at a time, 0 has no limit"
  >
    <span>{LIMITS.words.name} (0: no limit)</span>
    <input type="number" min="0" max="20" value={value.words} {disabled} onchange={(e) => number(e, "words")} />
  </label>
  <label class="field">
    <span>{LIMITS.seconds.name} (s)</span>
    <input
      type="number"
      min="1"
      max="15"
      step="0.5"
      value={value.seconds}
      {disabled}
      onchange={(e) => number(e, "seconds")}
    />
  </label>
{/if}
{#if shows("text")}<CaptionText {value} {disabled} {onChange} {onError} />{/if}

<style>
  .check {
    display: flex;
    gap: 8px;
    align-items: center;
    margin: 6px 0;
  }
  .field {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin: 6px 0;
  }
  .field {
    gap: 8px;
  }
  .field input[type="number"] {
    width: 72px;
  }
  .field input.name,
  .field select {
    width: 150px;
    min-width: 0;
  }
  .pair {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .hint {
    margin: 2px 0 6px;
    font-size: 11px;
    opacity: 0.75;
  }
  .small {
    font-size: 11px;
    padding: 2px 6px;
  }
  .muted {
    opacity: 0.7;
  }
  .field input[type="color"] {
    width: 40px;
    height: 24px;
    padding: 0;
  }
</style>
