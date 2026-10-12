<script lang="ts">
  import type { Progress, ProjectStatus, StageState } from "./api";
  import { clock } from "./time";

  let {
    status,
    error,
    analysis,
    outdated,
    onReanalyse,
  }: {
    status: ProjectStatus | null;
    error: string | null;
    /** The app's own analysis of this recording (2d), while it runs and after. */
    analysis: Progress | null;
    /** Why the analysis is out of date with the settings now. */
    outdated: string[];
    onReanalyse: () => void;
  } = $props();
  let collapsed = $state(false);

  const busy = $derived(analysis?.phase === "queued" || analysis?.phase === "running");

  // `running` in jobs.json is a stage started and not finished: running now while the app analyses,
  // else a run that was stopped (a closed app or a killed CLI).
  const label = (s: StageState): string =>
    ({
      pending: "not run",
      running: busy ? "running…" : "interrupted",
      done: "done",
      failed: "failed",
    })[s];
</script>

<aside class:collapsed>
  <button class="head" onclick={() => (collapsed = !collapsed)}>
    {collapsed ? "Project +" : "Project -"}
  </button>
  {#if !collapsed}
    {#if analysis && busy}
      <p class="analysing">
        {analysis.phase === "queued" ? "Waiting for another recording's analysis…" : "Analysing…"}
        {#if analysis.line}<span class="line" title={analysis.line}>{analysis.line}</span>{/if}
      </p>
    {:else if analysis?.phase === "failed"}
      <p class="failed" title={analysis.error ?? ""}>Analysis failed: {analysis.error}</p>
    {/if}
    {#if outdated.length > 0 && !busy}
      <div class="outdated">
        {#each outdated as why (why)}<p>{why}</p>{/each}
      </div>
    {/if}
    {#if error}
      <p class="failed">{error}</p>
    {:else if status}
      <ul>
        {#each status.stages as s (s.name)}
          <li><span>{s.name}</span><span class={s.state}>{label(s.state)}</span></li>
        {/each}
        <li>
          <span>transcript</span>
          <span class={status.transcript ? "done" : "pending"}>
            {status.transcript ? "yes" : "none"}
          </span>
        </li>
        <li>
          <span>edit list</span>
          {#if status.edit}
            <span class="done">
              {status.edit.ranges} ranges, {clock(status.edit.kept)} of {clock(status.edit.duration)}
            </span>
          {:else if status.editError}
            <span class="failed" title={status.editError}>unreadable</span>
          {:else}
            <span class="pending">none</span>
          {/if}
        </li>
      </ul>
      <p class="dir" title={status.dir}>{status.dir}</p>
      <button class="again" class:wanted={outdated.length > 0} disabled={busy} onclick={onReanalyse}
        >Re-analyse</button
      >
    {/if}
  {/if}
</aside>

<style>
  aside {
    position: absolute;
    top: 16px;
    left: 16px;
    width: 300px;
    padding: 8px 12px 10px;
    border-radius: 8px;
    background: var(--panel);
    border: 1px solid var(--line);
  }
  aside.collapsed {
    width: auto;
    padding-bottom: 8px;
  }
  .head {
    background: none;
    border: none;
    padding: 0;
    font-weight: 600;
  }
  ul {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
  }
  li {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    padding: 3px 0;
    border-top: 1px solid var(--line);
  }
  li span:first-child {
    color: var(--muted);
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
  .analysing {
    margin: 8px 0 0;
    color: var(--accent);
  }
  .line {
    display: block;
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  p.failed {
    margin: 8px 0 0;
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
  .again {
    margin-top: 8px;
    width: 100%;
  }
  .again.wanted {
    border-color: var(--accent);
    color: var(--accent);
  }
  .dir {
    margin: 8px 0 0;
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
