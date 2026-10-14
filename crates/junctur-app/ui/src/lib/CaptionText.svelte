<script lang="ts">
  // The caption text options (`CaptionRules`' `text` part). Its own component because the two lists
  // are edited as text, one entry per line; a line the engine would refuse snaps the box back.
  import { translationLanguages, type CaptionCase, type Captions, type CensorStyle, type Language } from "./api";
  import Field from "./Field.svelte";

  let {
    value,
    disabled = false,
    onChange,
    onError,
  }: {
    value: Captions;
    disabled?: boolean;
    onChange: (c: Captions) => void;
    onError: (message: string) => void;
  } = $props();

  let languages = $state<Language[]>([]);
  translationLanguages()
    .then((l) => (languages = l))
    .catch(() => (languages = []));

  const CASES: { key: CaptionCase; label: string }[] = [
    { key: "asSpoken", label: "As spoken" },
    { key: "upper", label: "UPPERCASE" },
    { key: "lower", label: "lowercase" },
    { key: "title", label: "Title Case" },
  ];
  const STYLES: { key: CensorStyle; label: string }[] = [
    { key: "middle", label: "Middle (f**k)" },
    { key: "whole", label: "Whole word (****)" },
  ];

  // The `captions` line's quotes and separators, and SRT tags (as the engine refuses them).
  const BAD = /["|<>{}]/;

  const lines = (el: HTMLTextAreaElement) =>
    el.value
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l);

  function censor(e: Event) {
    const el = e.currentTarget as HTMLTextAreaElement;
    const list = lines(el);
    const bad = list.find((w) => BAD.test(w) || !/[\p{L}\p{N}]/u.test(w));
    if (bad !== undefined) {
      el.value = value.censor.join("\n");
      onError(`"${bad}" cannot be censored: it needs a letter or digit and no " | < > { }`);
      return;
    }
    onChange({ ...value, censor: list });
  }

  function replace(e: Event) {
    const el = e.currentTarget as HTMLTextAreaElement;
    const pairs: { from: string; to: string }[] = [];
    for (const l of lines(el)) {
      const at = l.indexOf(">");
      const from = at < 0 ? "" : l.slice(0, at).trim();
      const to = at < 0 ? "" : l.slice(at + 1).trim();
      if (at < 0 || !/[\p{L}\p{N}]/u.test(from) || BAD.test(from) || BAD.test(to)) {
        el.value = shown(value.replace);
        onError(`"${l}" should read: words to find > what to show (no " | < { } besides the one >)`);
        return;
      }
      pairs.push({ from, to });
    }
    onChange({ ...value, replace: pairs });
  }

  const shown = (pairs: { from: string; to: string }[]) =>
    pairs.map((p) => `${p.from} > ${p.to}`).join("\n");
</script>

<Field label="Translate to" info="Shows the captions in another language, translated on this machine a sentence at a time; Off shows them as spoken. The transcript always stays as spoken.">
  {#snippet children(id)}
    <select
      {id}
      value={value.translate}
      {disabled}
      onchange={(e) => onChange({ ...value, translate: (e.currentTarget as HTMLSelectElement).value })}
    >
      <option value="">Off (as spoken)</option>
      {#if value.translate && !languages.some((l) => l.code === value.translate)}
        <option value={value.translate}>{value.translate}</option>
      {/if}
      {#each languages as l (l.code)}<option value={l.code}>{l.name}</option>{/each}
    </select>
  {/snippet}
</Field>
<Field label="Case">
  {#snippet children(id)}
    <select
      {id}
      value={value.case}
      {disabled}
      onchange={(e) => onChange({ ...value, case: (e.currentTarget as HTMLSelectElement).value as CaptionCase })}
    >
      {#each CASES as c (c.key)}<option value={c.key}>{c.label}</option>{/each}
    </select>
  {/snippet}
</Field>
<Field kind="check" label="Remove punctuation" info="Apostrophes and hyphens inside words stay.">
  {#snippet children(id)}
    <input
      {id}
      type="checkbox"
      checked={!value.punctuation}
      {disabled}
      onchange={(e) => onChange({ ...value, punctuation: !(e.currentTarget as HTMLInputElement).checked })}
    />
  {/snippet}
</Field>
<Field kind="stack" label="Censored words" info="Whole words or phrases, one per line, any case.">
  {#snippet children(id)}
    <textarea {id} rows="3" spellcheck="false" value={value.censor.join("\n")} {disabled} onchange={censor}></textarea>
  {/snippet}
</Field>
<Field label="Censor as">
  {#snippet children(id)}
    <select
      {id}
      value={value.censorStyle}
      {disabled}
      onchange={(e) =>
        onChange({ ...value, censorStyle: (e.currentTarget as HTMLSelectElement).value as CensorStyle })}
    >
      {#each STYLES as s (s.key)}<option value={s.key}>{s.label}</option>{/each}
    </select>
  {/snippet}
</Field>
<Field
  kind="stack"
  label="Find and replace"
  info="One per line: words to find > what to show. Whole words or phrases, any case, applied before case and censoring; nothing after > removes them."
>
  {#snippet children(id)}
    <textarea
      {id}
      rows="3"
      spellcheck="false"
      placeholder="gonna > going to"
      value={shown(value.replace)}
      {disabled}
      onchange={replace}
    ></textarea>
  {/snippet}
</Field>
