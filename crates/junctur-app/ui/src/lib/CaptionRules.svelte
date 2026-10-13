<script lang="ts">
  // The caption settings' fields (2i): moved out of the Captions panel so Settings edits the
  // defaults with the very same fields. Every field of `Captions` is here once, in three parts the
  // panel places between its own sections (`switches`: export and preview, 5d; `style`: bold and
  // italic, 5d, underline, 5f; `cut`: how cues are cut, 5a) and Settings shows together (`all`); a field added to
  // `Captions` goes into one part and shows in both. Number fields commit on change, inside the
  // engine's limits (core edit/captions.rs); anything else snaps back to `value`.
  import type { Captions } from "./api";

  type Part = "switches" | "style" | "cut" | "all";

  let {
    value,
    part = "all",
    disabled = false,
    onChange,
    onError,
  }: {
    value: Captions;
    part?: Part;
    disabled?: boolean;
    onChange: (c: Captions) => void;
    onError: (message: string) => void;
  } = $props();

  const shows = (p: Part) => part === "all" || part === p;

  const LIMITS = {
    chars: { min: 10, max: 80, name: "Characters per line" },
    lines: { min: 1, max: 3, name: "Lines per speaker" },
    seconds: { min: 1, max: 15, name: "Longest caption" },
  } as const;

  type Flag = "on" | "preview" | "bold" | "italic" | "underline";
  const SWITCHES: { key: Flag; label: string; title: string }[] = [
    { key: "on", label: "Export captions", title: "On: rendering also writes the captions (.srt) beside the video" },
    { key: "preview", label: "Preview captions", title: "Shows the captions over the video as it plays, as the export cuts them" },
  ];
  const STYLE: { key: Flag; label: string; title: string }[] = [
    { key: "bold", label: "Bold", title: "Every caption bold in the SRT" },
    { key: "italic", label: "Italic", title: "Every caption italic in the SRT" },
    { key: "underline", label: "Underline", title: "Every caption underlined in the SRT" },
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
    onChange({ ...value, [key]: key === "seconds" ? v : Math.round(v) });
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

{#if shows("switches")}{@render checks(SWITCHES)}{/if}
{#if shows("style")}{@render checks(STYLE)}{/if}
{#if shows("cut")}
  <label class="field">
    <span>{LIMITS.chars.name}</span>
    <input type="number" min="10" max="80" value={value.chars} {disabled} onchange={(e) => number(e, "chars")} />
  </label>
  <label class="field">
    <span>{LIMITS.lines.name}</span>
    <input type="number" min="1" max="3" value={value.lines} {disabled} onchange={(e) => number(e, "lines")} />
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
  .field input[type="number"] {
    width: 72px;
  }
</style>
