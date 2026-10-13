<script lang="ts">
  // The captions over the picture while the editor plays (5d): the Captions panel's "Preview
  // captions" toggle. It shows the cues the SRT export cuts (the `caption_cues` command runs the
  // same `srt::cues`), not a second cutting of the words here, so what is seen is what Resolve gets,
  // cue for cue: people talking at once in one caption a speaker per block (5e), each line in its
  // speaker's colour. Each block sits where the engine places it (5g, `export/layout.rs`: stacked,
  // side by side or top and bottom, the protagonist kept in place), which is where the Resolve send
  // puts that speaker's Text+, in the font, size and outline it gets.
  // Cues are on the edited (output) timeline, so the playhead's output time picks one; while the
  // whole recording plays, a cut stretch shows none, since it is not in the export.
  import * as api from "./api";
  import { captionFace, captionStyle } from "./captionStyle";
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

  // The cues, sorted and never overlapping (the export's `settle`), so a binary search finds the one
  // on screen. Asked again only when something that shapes them changes: the kept ranges, the frame
  // rate, how cues are cut, who says what (`speakersKey`: the turns, which follow a new transcript
  // or a re-hearing), and where blocks go (size, position, layout, protagonist, 5g). Font, outline,
  // bold, italic, underline, colours and names are drawn here from the edit state, so toggling them
  // costs no new cutting of a long transcript. A reply older than a newer request is dropped.
  const shape = $derived.by(() => {
    if (!on || !edit || !captions) return null;
    const fps = edit.fps ? `${edit.fps.num}/${edit.fps.den}` : "";
    // The picture's shape too: side by side, lines are wrapped to the columns' width.
    const aspect = picture.w > 0 ? (picture.h / picture.w).toFixed(3) : "";
    const place = `${captions.size}|${captions.position}|${captions.layout}|${captions.protagonist}|${aspect}`;
    return `${edit.ranges.join(",")}|${fps}|${captions.chars}|${captions.lines}|${captions.seconds}|${edit.speakersKey}|${place}`;
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
    api
      .captionCues(picture.w > 0 ? picture.h / picture.w : 9 / 16)
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
      <div class="part" style={style} style:left="{part.x * 100}%" style:top="{(1 - part.y) * 100}%">
        {#each part.lines as line, j (j)}
          <span
            class="line"
            style:color={colors.get(part.speaker) ?? "#FFFFFF"}
            style:text-decoration={decoration}>{line}</span
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
