<script lang="ts">
  // The captions over the picture while the editor plays: the cues the SRT export cuts, placed where
  // the engine places them, so what is seen is what Resolve gets, cue for cue.
  import { untrack } from "svelte";
  import * as api from "./api";
  import { animationProgress, animationStyle, captionFace, captionStyle, highlightColors } from "./captionStyle";
  import type { Rect } from "./layout";

  let {
    player,
    edit,
    picture,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    picture: Rect;
  } = $props();

  const captions = $derived(edit?.captions ?? null);
  const on = $derived(!!captions?.preview && !!edit?.editable);

  // The cues, sorted and never overlapping, so a binary search finds the one on screen. Asked again
  // only when something that shapes the cut changes; looks drawn from the edit state need no re-cut.
  // A reply older than a newer request is dropped.
  let translated = $state(0);
  let lastDone = 0;
  $effect(() => {
    const off = api.onTranslation((r) => {
      if (!r.running || r.done - lastDone >= 40 || r.done < lastDone) {
        lastDone = r.done;
        translated++;
      }
    });
    return () => {
      off.then((f) => f());
    };
  });
  const shape = $derived.by(() => {
    if (!on || !edit || !captions) return null;
    const fps = edit.fps ? `${edit.fps.num}/${edit.fps.den}` : "";
    const aspect = picture.w > 0 ? (picture.h / picture.w).toFixed(3) : "";
    // The highlight too: the engine only finds the words on the lines with it on.
    const place = `${captions.size}|${captions.position}|${captions.layout}|${captions.protagonist}|${aspect}|${captions.highlight}`;
    // JSON for the lists: entries may hold any separator a plain join would use.
    const text = JSON.stringify([
      captions.case,
      captions.punctuation,
      captions.censor,
      captions.censorStyle,
      captions.replace,
      captions.translate,
      translated,
    ]);
    return `${edit.ranges.join(",")}|${fps}|${captions.chars}|${captions.lines}|${captions.words}|${captions.seconds}|${edit.speakersKey}|${edit.fixesKey}|${place}|${text}`;
  });
  let cues = $state.raw<api.Cue[]>([]);
  let asked = 0;
  $effect(() => {
    if (shape == null) {
      asked++;
      cues = [];
      return;
    }
    const n = ++asked;
    // The aspect itself is read untracked: `shape` already holds it to three decimals, and reading
    // the picture's size here would re-cut the whole transcript on every tick of a resize.
    const aspect = untrack(() => (picture.w > 0 ? picture.h / picture.w : 9 / 16));
    api
      .captionCues(aspect)
      .then((c) => {
        if (n === asked) cues = c;
      })
      .catch(() => {
        // No transcript yet, or no frame rate: nothing to show until the next edit state.
        if (n === asked) cues = [];
      });
  });

  function find(t: number): api.Cue | null {
    let lo = 0;
    let hi = cues.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (cues[mid].start <= t) lo = mid + 1;
      else hi = mid;
    }
    const c = cues[lo - 1];
    return c && t < c.end ? c : null;
  }

  const cue = $derived.by(() => {
    if (!on || !player || player.inCut || player.output == null) return null;
    return find(player.output);
  });

  const colors = $derived(new Map((edit?.speakers ?? []).map((s) => [s.id, s.color])));

  type Part = api.Cue["parts"][number];

  /** The word lit at output second `t`, as Resolve's stepped highlight spline lights it: lit until
   * the next word starts, the last until the caption ends, the first from the caption's start. */
  function lit(part: Part, t: number): number | null {
    let w: number | null = part.words.length > 0 ? part.words[0].word : null;
    for (const s of part.words) if (s.start <= t) w = s.word;
    return w;
  }

  /** A line cut into runs, each lit or not, by the lit word's span on it. */
  function runs(part: Part, j: number, word: number | null): { text: string; on: boolean }[] {
    const line = part.lines[j];
    const spans = word == null ? [] : part.words.filter((s) => s.word === word && s.line === j);
    if (spans.length === 0) return [{ text: line, on: false }];
    const chars = Array.from(line);
    const out: { text: string; on: boolean }[] = [];
    let at = 0;
    for (const s of spans) {
      if (s.from > at) out.push({ text: chars.slice(at, s.from).join(""), on: false });
      out.push({ text: chars.slice(s.from, s.to).join(""), on: true });
      at = s.to;
    }
    if (at < chars.length) out.push({ text: chars.slice(at).join(""), on: false });
    return out;
  }

  const highlighting = $derived(!!captions?.highlight);
  // With no animation the blocks' style is fixed: worked out without reading the playhead, so it
  // is not redone on every tick.
  const still = animationStyle({ animation: "none" }, 1);
  const motion = $derived.by(() => {
    if (!captions || captions.animation === "none") return still;
    if (!cue || player?.output == null) return still;
    return animationStyle(captions, animationProgress(captions, cue.start, cue.end, player.output));
  });
  const style = $derived(captions ? captionStyle(captions, picture.h) : "");
  const decoration = $derived(captions ? captionFace(captions).decoration : "none");

  // One log line per caption shown, so a test-mode run can tell the preview drew (the report's `ui`).
  // Keyed by time, not the object: the cues are asked for again on every edit state.
  let logged = "";
  $effect(() => {
    const key = cue ? `${cue.start}-${cue.end}` : "";
    // Cleared between captions too, so coming back to the same one (after a cut) logs it again.
    if (!cue) logged = "";
    else if (key !== logged) {
      logged = key;
      const text = cue.parts.map((p) => `${p.speaker}:${p.lines.join(" / ")}`).join(" | ");
      void api.uiLog(`caption preview ${cue.start.toFixed(2)}-${cue.end.toFixed(2)}: ${text}`).catch(() => {});
    }
  });
</script>

{#if cue}
  <div
    class="captions"
    style:left="{picture.x}px"
    style:top="{picture.y}px"
    style:width="{picture.w}px"
    style:height="{picture.h}px"
    aria-live="off"
  >
    {#each cue.parts as part, i (i)}
      {@const own = colors.get(part.speaker) ?? "#FFFFFF"}
      {@const [base, word] = highlighting && captions ? highlightColors(captions, own) : [own, own]}
      {@const on = highlighting ? lit(part, player?.output ?? 0) : null}
      <div
        class="part"
        style={style}
        style:left="{part.x * 100}%"
        style:top="{(1 - part.y) * 100}%"
        style:opacity={motion.opacity}
        style:transform={motion.transform}
      >
        {#each part.lines as line, j (j)}
          <span class="line" style:color={base} style:text-decoration={decoration}
            >{#if highlighting}{#each runs(part, j, on) as r, k (k)}{#if r.on}<span style:color={word}
                    >{r.text}</span
                  >{:else}{r.text}{/if}{/each}{:else}{line}{/if}</span
          >
        {/each}
      </div>
    {/each}
  </div>
{/if}

<style>
  .captions {
    position: absolute;
    pointer-events: none;
    overflow: hidden;
  }
  /* Centred on the engine's point, as Text+ centres its text on Center. */
  .part {
    position: absolute;
    transform: translate(-50%, -50%);
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
  }
  .line {
    white-space: pre;
  }
</style>
