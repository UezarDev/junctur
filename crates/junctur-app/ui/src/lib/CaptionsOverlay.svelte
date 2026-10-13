<script lang="ts">
  // The captions over the picture while the editor plays (5d): the Captions panel's "Preview
  // captions" toggle. It shows the cues the SRT export cuts (the `caption_cues` command runs the
  // same `srt::cues`), not a second cutting of the words here, so what is seen is what Resolve gets,
  // cue for cue: people talking at once stacked in one caption a speaker per block (5e), each line in
  // its speaker's colour, and the styling options as close as a browser draws Resolve's subtitles.
  // Cues are on the edited (output) timeline, so the playhead's output time picks one; while the
  // whole recording plays, a cut stretch shows none, since it is not in the export.
  import * as api from "./api";
  import { captionStyle } from "./captionStyle";
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
  // on screen. Asked again whenever the edit state is replaced (an edit, a re-cut, new speakers or
  // settings, the transcript arriving); a request that comes back after a newer one is dropped.
  let cues = $state.raw<api.Cue[]>([]);
  let asked = 0;
  $effect(() => {
    if (!on || !edit) {
      cues = [];
      return;
    }
    void edit;
    const n = ++asked;
    api
      .captionCues()
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

  // One log line per caption shown, so a test-mode run can tell the preview drew (the report's `ui`).
  // Keyed by time, not the object: the cues are asked for again on every edit state.
  let logged = "";
  $effect(() => {
    const key = cue ? `${cue.start}-${cue.end}` : "";
    if (cue && key !== logged) {
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
    <div class="cue" style={style}>
      {#each cue.parts as part, i (i)}
        {#each part.lines as line, j (j)}
          <span class="line" style:color={colors.get(part.speaker) ?? "#FFFFFF"}>{line}</span>
        {/each}
      {/each}
    </div>
  </div>
{/if}

<style>
  .captions {
    position: absolute;
    display: flex;
    align-items: flex-end;
    justify-content: center;
    pointer-events: none;
    overflow: hidden;
  }
  .cue {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    max-width: 90%;
  }
  .line {
    white-space: pre;
  }
</style>
