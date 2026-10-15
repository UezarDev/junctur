<script lang="ts">
  // The Properties tab: the recording, the background transcription job, the analysis steps with
  // their progress, the out-of-date notice, the speakers and the project folder. Analysing again is
  // Correct's pop-up (the transcript's word bar), so it has no button of its own here.
  import * as api from "./api";
  import { clock } from "./time";
  import { busy as running, current, whole } from "./steps";
  import { name as trackName } from "./tracks";
  import TrackSteps from "./TrackSteps.svelte";
  import TranscriptionJob from "./TranscriptionJob.svelte";

  let {
    player,
    edit,
    mixer,
    status,
    error,
    analysis,
    lane,
    outdated,
    waves,
    onError,
  }: {
    player: api.PlayerState | null;
    edit: api.EditState | null;
    mixer: api.MixerState | null;
    status: api.ProjectStatus | null;
    /** The project could not be opened (the file still plays). */
    error: string | null;
    analysis: api.Progress | null;
    /** The transcription lane, whichever recording is open. */
    lane: api.Transcription | null;
    outdated: string[];
    waves: api.WavesState | null;
    onError: (message: string) => void;
  } = $props();

  const busy = $derived(running(analysis));
  const NAMES: Record<string, string> = {
    tracks: "Tracks",
    speech: "Speech",
    speakers: "Speakers",
    transcribe: "Transcript",
    edit: "Edit list",
  };
  // The running stage's tracks, named as their lanes are.
  const steps = $derived(current(analysis));
  function trackLabel(track: number): string {
    const t = mixer?.tracks.find((m) => m.index === track);
    return t ? `${track + 1} ${trackName(t, player?.audioTracks ?? [])}` : `Track ${track + 1}`;
  }
  const total = $derived(whole(steps));
  const file = $derived(player?.path ? (player.path.split(/[\\/]/).pop() ?? player.path) : "");
  const fps = $derived(edit?.fps ? edit.fps.num / edit.fps.den : null);
  const size = $derived(player?.width && player?.height ? `${player.width}×${player.height}` : null);
  const length = $derived(
    [
      clock(player?.duration),
      fps ? `${Number.isInteger(fps) ? fps : fps.toFixed(2)} fps` : null,
      size,
    ]
      .filter(Boolean)
      .join(" · "),
  );

  // `running` in jobs.json is a stage started and not finished: running now while the app
  // analyses, else a run that was stopped (a closed app or a killed CLI).
  function state(s: api.StageState): { label: string; cls: string } {
    switch (s) {
      case "done":
        return { label: "done", cls: "done" };
      case "failed":
        return { label: "failed", cls: "failed" };
      case "running":
        return busy ? { label: "running…", cls: "running" } : { label: "interrupted", cls: "failed" };
      default:
        return { label: busy ? "waiting" : "not run", cls: "pending" };
    }
  }

  async function reveal() {
    try {
      await api.reveal();
    } catch (e) {
      onError(String(e));
    }
  }
</script>

<div class="props">
  <section>
    <h3>Recording</h3>
    <dl>
      <dt>File</dt>
      <dd title={player?.path ?? ""}>{file}</dd>
      <dt>Length</dt>
      <dd class="mono">{length}</dd>
      <dt>Kept after cuts</dt>
      <dd>
        {#if status?.edit}
          <span class="mono">{clock(status.edit.kept)}</span> in {status.edit.ranges} ranges
        {:else if status?.editError}
          <span class="failed" title={status.editError}>edit list unreadable</span>
        {:else}
          <span class="pending">after the edit list</span>
        {/if}
      </dd>
      <dt>Audio tracks</dt>
      <dd>{mixer?.tracks.length ?? player?.audioTracks.length ?? 0}</dd>
    </dl>
  </section>

  {#if lane && lane.jobs.length > 0}
    <TranscriptionJob {lane} open={analysis?.file ?? null} {onError} />
  {/if}

  <section>
    <h3>
      Analysis
      {#if analysis?.phase === "queued"}<span class="note">waiting for another recording</span>{/if}
      {#if analysis?.phase === "waiting"}<span class="note">transcription waits for another recording</span>{/if}
    </h3>
    {#if error}
      <p class="failed">{error}</p>
    {:else if status}
      <ul class="stages">
        {#each status.stages as s (s.name)}
          {@const st = state(s.state)}
          {@const mine = st.cls === "running" && steps.length > 0 && steps[0].stage === s.name}
          <li>
            <span class="icon {st.cls}" aria-hidden="true"></span>
            <span class="nm">{NAMES[s.name] ?? s.name}</span>
            <span class="st {st.cls}">
              {#if mine && total != null}{total}%{:else}{st.label}{/if}
            </span>
            {#if mine}
              <span class="tracks"><TrackSteps {steps} name={trackLabel} /></span>
            {/if}
          </li>
        {/each}
      </ul>
      {#if analysis && busy && analysis.line}
        <p class="line" title={analysis.line}>{analysis.line}</p>
      {:else if analysis?.phase === "failed"}
        <p class="failed" title={analysis.error ?? ""}>Analysis failed: {analysis.error}</p>
      {/if}
      {#if outdated.length > 0 && !busy}
        <div class="outdated">
          {#each outdated as why (why)}<p>{why}</p>{/each}
          <p class="how">Correct, under the transcript, analyses it again with the settings now.</p>
        </div>
      {/if}
    {/if}
  </section>

  <section>
    <h3>Speakers</h3>
    {#if edit?.speakers.length}
      <ul class="people">
        {#each edit.speakers as s (s.id)}
          <li>
            <span class="dot" style:background={s.color}></span>
            <span class="nm">{s.name}</span>
            <span class="where">{s.track != null ? `track ${s.track + 1}` : "by voice"}</span>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="pending">{busy ? "after the speakers step" : "none found"}</p>
    {/if}
  </section>

  <section>
    <h3>Waveforms</h3>
    {#if !waves}
      <p class="pending">not read</p>
    {:else if waves.error}
      <p class="failed" title={waves.error}>could not be read: {waves.error}</p>
    {:else if waves.done}
      <p class="done">{waves.tracks} {waves.tracks === 1 ? "track" : "tracks"}, read</p>
    {:else}
      <p class="running">
        reading{#if waves.duration} · {Math.min(99, Math.floor((waves.seconds / waves.duration) * 100))}%{/if}
      </p>
    {/if}
  </section>

  {#if status}
    <section>
      <h3>Project folder</h3>
      <p class="dir mono" title={status.dir}>{status.dir}</p>
      <button onclick={reveal}>Open folder</button>
    </section>
  {/if}
</div>

<style>
  .props {
    height: 100%;
    overflow-y: auto;
    padding: 4px 14px 16px;
    box-sizing: border-box;
    user-select: text;
    border-left: 1px solid var(--line);
  }
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
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    margin: 0;
  }
  dt,
  dd {
    margin: 0;
    padding: 4px 0;
    border-bottom: 1px solid var(--line);
  }
  dt {
    color: var(--muted);
    padding-right: 12px;
  }
  dd {
    text-align: right;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .stages li {
    display: grid;
    grid-template-columns: 18px 1fr auto;
    align-items: center;
    gap: 6px;
    padding: 4px 0;
  }
  .icon {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 1.5px dashed var(--muted);
    box-sizing: border-box;
  }
  .icon.done {
    border: none;
    background: var(--good);
    mask: radial-gradient(circle, #000 60%, transparent 62%);
  }
  .icon.running {
    border: 2px solid var(--accent);
    border-right-color: transparent;
    animation: spin 0.9s linear infinite;
  }
  .icon.failed {
    border: none;
    background: var(--bad);
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .tracks {
    grid-column: 2 / 4;
  }
  .st {
    font-size: 12px;
  }
  .done {
    color: var(--good);
  }
  .pending {
    color: var(--muted);
  }
  .running {
    color: var(--accent);
  }
  .failed {
    color: var(--bad);
  }
  p {
    margin: 4px 0;
  }
  .line {
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  p.failed {
    overflow-wrap: anywhere;
  }
  .outdated {
    margin: 8px 0 0;
    padding: 6px 8px;
    border-radius: 6px;
    border: 1px solid var(--accent);
  }
  .outdated p {
    margin: 0;
  }
  .outdated .how {
    margin-top: 4px;
    color: var(--muted);
    font-size: 12px;
  }
  .people li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
    border-bottom: 1px solid var(--line);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  .people .nm {
    flex: 1;
  }
  .where {
    color: var(--muted);
    font-size: 12px;
  }
  .dir {
    font-size: 12px;
    color: var(--muted);
    overflow-wrap: anywhere;
  }
</style>
