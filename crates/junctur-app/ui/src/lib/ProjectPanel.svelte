<script lang="ts">
  import type { ProjectStatus, StageState } from "./api";
  import { clock } from "./time";

  let { status, error }: { status: ProjectStatus | null; error: string | null } = $props();
  let collapsed = $state(false);

  const LABEL: Record<StageState, string> = {
    pending: "not run",
    running: "interrupted",
    done: "done",
    failed: "failed",
  };
</script>

<aside class:collapsed>
  <button class="head" onclick={() => (collapsed = !collapsed)}>
    {collapsed ? "Project +" : "Project -"}
  </button>
  {#if !collapsed}
    {#if error}
      <p class="failed">{error}</p>
    {:else if status}
      <ul>
        {#each status.stages as s (s.name)}
          <li><span>{s.name}</span><span class={s.state}>{LABEL[s.state]}</span></li>
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
    {/if}
  {/if}
</aside>

<style>
  aside {
    position: absolute;
    top: 16px;
    right: 16px;
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
  .dir {
    margin: 8px 0 0;
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
