<script lang="ts">
  // One row per track for the running analysis stage (2h): the track, a bar and its percentage.
  // Full rows in Properties; `compact` (bars only, the percentage in the tooltip) under the chip on
  // the picture and in a start-window row, where there is no room for names.
  import type * as api from "./api";
  import { percent } from "./steps";

  let {
    steps,
    name = (track: number) => `Track ${track + 1}`,
    compact = false,
  }: {
    steps: api.Step[];
    /** What a track is called; the start window knows no roles. */
    name?: (track: number) => string;
    compact?: boolean;
  } = $props();
</script>

<ul class="steps" class:compact>
  {#each steps as s (s.track)}
    {@const pct = percent(s)}
    <li title="{name(s.track)}: {pct}%">
      {#if !compact}<span class="nm">{name(s.track)}</span>{/if}
      <span class="bar"><span style:width="{pct}%"></span></span>
      {#if !compact}<span class="pct mono">{pct}%</span>{/if}
    </li>
  {/each}
</ul>

<style>
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li {
    display: grid;
    grid-template-columns: minmax(0, 7em) 1fr 3.2em;
    align-items: center;
    gap: 8px;
    padding: 2px 0;
    font-size: 12px;
  }
  .compact li {
    display: block;
    padding: 1px 0;
  }
  .nm {
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pct {
    text-align: right;
    color: var(--accent);
  }
  .bar {
    display: block;
    height: 4px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }
  .compact .bar {
    height: 3px;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.3s;
  }
</style>
