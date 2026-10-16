<script lang="ts">
  // The Moments tab (7a, a prototype): every moment the streamer marked while playing, by saying a
  // phrase or pressing a hotkey, as a list to work through instead of a long timeline: Jump plays
  // it, Accept keeps its half minute and cuts the phrase (one undo step), Reject leaves the list as
  // it is, Undo puts an accepted one back. Also where the speaker roll call opens again.
  import * as api from "./api";
  import { clock } from "./time";

  let {
    view,
    editable,
    focus,
    onAnswer,
    onJump,
    onRollCall,
  }: {
    view: api.MomentsView | null;
    /** The list can be edited now (not while the recording's run is making it). */
    editable: boolean;
    /** The moment last clicked on the lane, shown lit. */
    focus: number | null;
    onAnswer: (action: api.MomentAction, m: api.Moment | null) => Promise<void>;
    onJump: (m: api.Moment) => void;
    onRollCall: () => void;
  } = $props();

  const all = $derived(view?.moments ?? []);
  const waiting = $derived(all.filter((m) => m.state === "suggested").length);
  let showRejected = $state(false);
  const listed = $derived(showRejected ? all : all.filter((m) => m.state !== "rejected"));
  let busy = $state(false);

  async function act(action: api.MomentAction, m: api.Moment | null) {
    if (busy) return;
    busy = true;
    try {
      await onAnswer(action, m);
    } finally {
      busy = false;
    }
  }
  // The lit row comes into view when it is clicked on the lane.
  let rows: Record<number, HTMLElement | undefined> = {};
  $effect(() => {
    if (focus != null) rows[focus]?.scrollIntoView({ block: "nearest" });
  });
</script>

<div class="moments">
  <div class="top">
    <h3>Moments</h3>
    <button onclick={onRollCall} title="Name the voices of this recording once, up front">Roll call</button>
  </div>
  <p class="hint">
    Say {view?.phrases.slice(0, 3).map((p) => `“${p}”`).join(", ") ?? "“clip that”"} or “marca eso” while you play, or
    keep a markers file ({view?.markersHint ?? "<recording>.markers.txt beside the recording"}: one time per line), and
    each becomes the half minute before it.
  </p>
  {#if view?.markersFile}
    <p class="hint mono" title={view.markersFile}>
      Markers file read{view.markersBad ? `, ${view.markersBad} line${view.markersBad === 1 ? "" : "s"} not understood` : ""}.
    </p>
  {/if}
  {#if view?.markersError}
    <p class="hint bad">The markers file could not be read: {view.markersError}</p>
  {/if}
  {#if !view}
    <p class="hint">Reading the moments…</p>
  {:else if !all.length}
    <p class="empty">
      {view.heard ? "No moments marked in this recording yet." : "No transcript yet: spoken markers show once the words are in."}
    </p>
  {:else}
    <div class="row">
      <span class="count">{waiting} waiting · {all.length} in all</span>
      <button class="primary" disabled={!editable || busy || waiting === 0} onclick={() => act("acceptAll", null)}>Accept all</button>
    </div>
    <ul>
      {#each listed as m (m.at)}
        <li class:lit={focus === m.at} class:done={m.state !== "suggested"} bind:this={rows[m.at]}>
          <button class="when mono" onclick={() => onJump(m)} title="Jump to it and play">{clock(m.start)}</button>
          <span class="what">
            <span class="label">{m.label}</span>
            <span class="sub"
              >{m.source === "voice" ? "said" : m.source === "like" ? "like this" : "hotkey"} at {clock(m.at)} · {Math.round(m.end - m.start)} s{m.state === "accepted"
                ? " · kept"
                : m.state === "rejected"
                  ? " · rejected"
                  : ""}</span
            >
          </span>
          {#if m.state === "suggested"}
            <button class="primary" disabled={!editable || busy} onclick={() => act("accept", m)}>Accept</button>
            <button disabled={!editable || busy} onclick={() => act("reject", m)}>Reject</button>
          {:else}
            <button disabled={!editable || busy} onclick={() => act("undo", m)}>Undo</button>
          {/if}
        </li>
      {/each}
    </ul>
    {#if all.some((m) => m.state === "rejected")}
      <label class="hint"><input type="checkbox" bind:checked={showRejected} /> Show rejected</label>
    {/if}
  {/if}
</div>

<style>
  .moments {
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .top,
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
  }
  .hint,
  .empty,
  .count {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }
  .bad {
    color: var(--bad);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 6px;
    border-radius: 6px;
    border-left: 3px solid var(--warn);
    background: var(--raised);
  }
  li.done {
    border-left-color: var(--line);
  }
  li.lit {
    outline: 1px solid var(--warn);
  }
  .when {
    padding: 1px 6px;
  }
  .what {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    font-size: 11px;
    color: var(--muted);
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
