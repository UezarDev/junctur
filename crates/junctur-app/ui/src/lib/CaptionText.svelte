<script lang="ts">
  // The caption text options (5j), `CaptionRules`' `text` part: the language captions are shown in
  // (5k: translated on this machine; the Captions panel runs the translation), case, punctuation,
  // censored words and find-and-replace pairs, which apply to the translated words. A component of
  // its own because the two lists are edited as text, one entry per line, with their own parsing. They change only what the captions show (the SRT, Send to
  // Resolve, the preview), never the transcript. A list commits when the box loses focus; a line
  // the engine would refuse (core edit/caption_text.rs) is reported and the box snaps back.
  import { translationLanguages, type CaptionCase, type Captions, type CensorStyle, type Language } from "./api";

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
    { key: "middle", label: "Middle masked (f**k)" },
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

<label class="field" title="Captions in another language, translated on this machine; the transcript stays as spoken">
  <span>Translate to</span>
  <select
    value={value.translate}
    {disabled}
    onchange={(e) => onChange({ ...value, translate: (e.currentTarget as HTMLSelectElement).value })}
  >
    <option value="">As spoken (no translation)</option>
    {#if value.translate && !languages.some((l) => l.code === value.translate)}
      <option value={value.translate}>{value.translate}</option>
    {/if}
    {#each languages as l (l.code)}<option value={l.code}>{l.name}</option>{/each}
  </select>
</label>
<label class="field">
  <span>Case</span>
  <select
    value={value.case}
    {disabled}
    onchange={(e) => onChange({ ...value, case: (e.currentTarget as HTMLSelectElement).value as CaptionCase })}
  >
    {#each CASES as c (c.key)}<option value={c.key}>{c.label}</option>{/each}
  </select>
</label>
<label class="check" title="Shows captions without punctuation; apostrophes and hyphens inside words stay">
  <input
    type="checkbox"
    checked={!value.punctuation}
    {disabled}
    onchange={(e) => onChange({ ...value, punctuation: !(e.currentTarget as HTMLInputElement).checked })}
  />
  Remove punctuation
</label>
<label class="list" title="Whole words or phrases, one per line, case aside">
  <span>Censored words (one per line)</span>
  <textarea rows="3" value={value.censor.join("\n")} {disabled} onchange={censor}></textarea>
</label>
<label class="field">
  <span>Censor as</span>
  <select
    value={value.censorStyle}
    {disabled}
    onchange={(e) =>
      onChange({ ...value, censorStyle: (e.currentTarget as HTMLSelectElement).value as CensorStyle })}
  >
    {#each STYLES as s (s.key)}<option value={s.key}>{s.label}</option>{/each}
  </select>
</label>
<label
  class="list"
  title="One per line: words to find > what to show. Whole words or phrases, case aside, before case and censoring; nothing after > removes them"
>
  <span>Find and replace (find &gt; show)</span>
  <textarea rows="3" placeholder="gonna > going to" value={shown(value.replace)} {disabled} onchange={replace}></textarea>
</label>

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
    gap: 8px;
    margin: 6px 0;
  }
  .field select {
    width: 150px;
    min-width: 0;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 6px 0;
  }
  .list textarea {
    resize: vertical;
    font: inherit;
  }
</style>
