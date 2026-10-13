<script lang="ts">
  // The transcription job in the Properties tab (2k): the one shared background job (2j) seen from
  // whichever recording is open, with Pause / Resume (Uezar: "a panel inside properties to see the
  // current transcription job if the user changed to another video, and the ability to pause it").
  // Pause is the lane's: the transcribing recording stops after the chunk it is on and the ones
  // waiting keep waiting. Its own component because Properties is the open recording's and this is
  // not.
  import * as api from "./api";
  import { clock } from "./time";
  import { current, paused, whole } from "./steps";
  import TrackSteps from "./TrackSteps.svelte";

  let {
    lane,
    open,
    onError,
  }: {
    lane: api.Transcription;
    /** The open recording's key, when it has a run: it is "this recording" here. */
    open: string | null;
    onError: (message: string) => void;
  } = $props();

  const head = $derived(lane.jobs[0]);
  const waiting = $derived(lane.jobs.slice(1));
  const mine = $derived(head.file === open);
  // The head may still be between its edit list and Whisper's first chunk: no steps yet.
  const steps = $derived(current(head));
  const pct = $derived(whole(steps));
  const pause = $derived(paused(lane, head.file));
  let asking = $state(false);

  function base(file: string): string {
    return file.split(/[\\/]/).pop() ?? file;
  }

  async function toggle() {
    asking = true;
    try {
      await api.pauseTranscription(!lane.paused);
    } catch (e) {
      onError(String(e));
    } finally {
      asking = false;
    }
  }
</script>

<section class="job">
  <h3>
    Transcription
    <span class="note" class:held={pause}>
      {pause === "paused" ? "paused" : pause === "pausing" ? "pausing after this chunk…" : "running"}
    </span>
  </h3>
  <div class="head">
    <span class="nm" title={head.file}>{mine ? "This recording" : base(head.file)}</span>
    <span class="pct mono">{pct != null ? `${pct}%` : "starting…"}</span>
  </div>
  <!-- The open recording's bars are under its Transcript step, just below. -->
  {#if !mine && steps.length > 0}
    <TrackSteps {steps} />
  {/if}
  {#if head.transcribed != null && head.transcribed > 0}
    <p class="through">Transcribed to <span class="mono">{clock(head.transcribed)}</span></p>
  {/if}
  <button class="pause" class:held={lane.paused} disabled={asking} onclick={toggle}>
    {lane.paused ? "Resume transcription" : "Pause transcription"}
  </button>
  {#if waiting.length > 0}
    <p class="label">Waiting</p>
    <ol class="queue">
      <!-- Keyed by place: a recording should be on the lane once, but a duplicate must not break the list. -->
      {#each waiting as w, i (i)}
        <li title={w.file}>{w.file === open ? "This recording" : base(w.file)}</li>
      {/each}
    </ol>
  {/if}
</section>

<style>
  section {
    margin-top: 14px;
  }
  h3 {
    display: flex;
    justify-content: space-between;
    margin: 0 0 6px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
  }
  .note {
    text-transform: none;
    letter-spacing: 0;
    color: var(--accent);
  }
  .note.held {
    color: var(--muted);
  }
  .head {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 0;
  }
  .head .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pct {
    color: var(--accent);
    font-size: 12px;
  }
  p {
    margin: 4px 0;
  }
  .through {
    color: var(--muted);
    font-size: 12px;
  }
  .pause {
    margin-top: 8px;
    width: 100%;
  }
  .pause.held {
    border-color: var(--accent);
    color: var(--accent);
  }
  .label {
    margin-top: 8px;
    color: var(--muted);
    font-size: 12px;
  }
  .queue {
    margin: 0;
    padding-left: 18px;
    font-size: 12px;
  }
  .queue li {
    padding: 2px 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
