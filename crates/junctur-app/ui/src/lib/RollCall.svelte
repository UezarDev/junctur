<script lang="ts">
  // The speaker roll call (7a, a prototype): a full-window screen with one big card per voice found
  // (a few seconds of it to play, on its own track, and a name box), then "Did we miss anyone?" for the
  // stretches the speakers are least sure of. Names are fixed once, up front, instead of corrected all
  // over the timeline: Done renames each speaker everywhere (as the transcript panel's rename does) and
  // remembers the ticked voices. Opens by itself once per recording; the Moments tab opens it again.
  import { onMount } from "svelte";
  import * as api from "./api";
  import { clock } from "./time";

  let {
    file,
    onClose,
    onError,
  }: {
    /** The open recording. */
    file: string;
    /** Closed; `lines` are what remembering voices said. */
    onClose: (lines: string[]) => void;
    onError: (message: string) => void;
  } = $props();

  let view = $state.raw<api.RollCallView | null>(null);
  let names = $state<Record<number, string>>({});
  let remember = $state<Record<number, boolean>>({});
  // What is playing: a card's sample ("c<id>") or an unsure stretch ("u<start>").
  let playing = $state<string | null>(null);
  let soloed: number | null = null;
  // Playing puts the player in review (a loop); leaving the roll call takes it out again.
  let played = false;
  let busy = $state(false);
  // Each unsure stretch's pick: a speaker id, "new", or "" for none yet; and the new person's name.
  let picks = $state<Record<number, string>>({});
  let newNames = $state<Record<number, string>>({});

  async function load(first: boolean) {
    try {
      const v = await api.rollCall(file);
      view = v;
      if (first) {
        for (const c of v.cards) {
          names[c.id] = c.unnamed ? "" : c.name;
          remember[c.id] = false;
        }
      }
    } catch (e) {
      onError(String(e));
    }
  }
  onMount(() => {
    void load(true);
    return () => void stop();
  });

  async function solo(track: number | null) {
    if (soloed === track) return;
    if (soloed != null) await api.setSolo(soloed, false);
    soloed = track;
    if (track != null) await api.setSolo(track, true);
  }
  async function stop() {
    if (playing == null && soloed == null) return;
    playing = null;
    try {
      await api.pauseReview();
      await solo(null);
    } catch (e) {
      onError(String(e));
    }
  }
  // Plays `a..b` on a loop on `track`, or stops it when it is what plays.
  async function play(key: string, a: number, b: number, track: number | null) {
    if (playing === key) return stop();
    try {
      await solo(track);
      await api.playBetween(a, b);
      played = true;
      playing = key;
    } catch (e) {
      onError(String(e));
    }
  }
  const voiceTrack = $derived(view?.cards.find((c) => c.track != null)?.track ?? null);
  const nameOf = (id: number | null) => view?.cards.find((c) => c.id === id)?.name ?? null;

  async function assign(u: api.Unsure) {
    const pick = picks[u.start] ?? "";
    const isNew = pick === "new";
    const name = (newNames[u.start] ?? "").trim();
    if (!pick || (isNew && !name)) return;
    busy = true;
    try {
      await api.rollCallAssign(u.start, u.end, isNew ? null : Number(pick), isNew ? name : null);
      await load(false);
      for (const c of view?.cards ?? []) if (!(c.id in names)) names[c.id] = c.name;
    } catch (e) {
      onError(String(e));
    } finally {
      busy = false;
    }
  }

  async function finish(save: boolean) {
    busy = true;
    const lines: string[] = [];
    try {
      await stop();
      if (played) await api.endReview();
      if (save && view) {
        for (const c of view.cards) {
          const name = (names[c.id] ?? "").trim();
          if (name && name !== c.name) await api.renameSpeaker(c.id, name);
        }
        for (const c of view.cards) {
          const name = (names[c.id] ?? "").trim() || (c.unnamed ? "" : c.name);
          if (remember[c.id] && name && c.voice.length) lines.push(await api.rollCallRemember(name, c.voice));
        }
      }
      await api.rollCallSeen(file);
      onClose(lines);
    } catch (e) {
      onError(String(e));
      busy = false;
    }
  }
</script>

<div class="screen" role="dialog" aria-label="Speaker roll call">
  <div class="inner">
    <h2>Who's who?</h2>
    <p class="hint">
      Play each voice and give it the name you use for them. A name is set everywhere in this recording; tick "Remember
      this voice" and Junctur names them in your next recordings too.
    </p>
    {#if !view}
      <p class="hint">Reading the speakers…</p>
    {:else if !view.cards.length}
      <p class="hint">No speakers were found in this recording yet.</p>
    {:else}
      <div class="cards">
        {#each view.cards as c (c.id)}
          <div class="card" style:--who={c.color}>
            <button
              class="play"
              class:on={playing === `c${c.id}`}
              disabled={!c.sample}
              onclick={() => c.sample && play(`c${c.id}`, c.sample[0], c.sample[1], c.track)}
              aria-label={playing === `c${c.id}` ? "Stop" : "Play this voice"}
            >
              {playing === `c${c.id}` ? "■" : "▶"}
            </button>
            <div class="who">
              <span class="dot"></span>
              <span class="was">{c.unnamed ? c.name : c.person ? `${c.name} (remembered)` : c.name}</span>
              <span class="sub">{Math.round(c.seconds)} s of talk{c.sample ? ` · plays ${clock(c.sample[0])}` : ""}</span>
            </div>
            <input
              class="name"
              placeholder="What's this person's nickname?"
              maxlength="60"
              bind:value={names[c.id]}
              onkeydown={(e) => e.stopPropagation()}
            />
            {#if view.canRemember}
              <label class="tick">
                <input
                  type="checkbox"
                  bind:checked={remember[c.id]}
                  disabled={(!(names[c.id] ?? "").trim() && c.unnamed) || !c.voice.length}
                />
                Remember this voice
              </label>
            {/if}
          </div>
        {/each}
      </div>
      {#if view.unsure.length}
        <h3>Did we miss anyone?</h3>
        <p class="hint">Stretches the speakers are least sure of. Play one and say who it is.</p>
        <ul class="unsure">
          {#each view.unsure as u (u.start)}
            <li>
              <button
                class:on={playing === `u${u.start}`}
                onclick={() => play(`u${u.start}`, u.start, u.end, u.track ?? voiceTrack)}
                aria-label="Play this stretch">{playing === `u${u.start}` ? "■" : "▶"}</button
              >
              <span class="mono">{clock(u.start)}</span>
              <span class="now"
                >{u.why === "nobody" ? "nobody yet" : `now ${nameOf(u.speaker) ?? "?"}, a short stretch between someone else`}</span
              >
              <select bind:value={picks[u.start]} onkeydown={(e) => e.stopPropagation()}>
                <option value="">Who is it?</option>
                {#each view.cards as c (c.id)}
                  <option value={String(c.id)}>{(names[c.id] ?? "").trim() || c.name}</option>
                {/each}
                <option value="new">New person…</option>
              </select>
              {#if picks[u.start] === "new"}
                <input
                  class="newname"
                  placeholder="Their name"
                  maxlength="60"
                  bind:value={newNames[u.start]}
                  onkeydown={(e) => e.stopPropagation()}
                />
              {/if}
              <button disabled={busy || !picks[u.start]} onclick={() => assign(u)}>Set</button>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
    <div class="end">
      <button onclick={() => finish(false)} disabled={busy}>Skip</button>
      <button class="primary" onclick={() => finish(true)} disabled={busy || !view}>Done</button>
    </div>
  </div>
</div>

<style>
  .screen {
    position: absolute;
    inset: 0;
    z-index: 45;
    background: var(--panel-solid);
    overflow-y: auto;
    user-select: text;
  }
  .inner {
    max-width: 1100px;
    margin: 0 auto;
    padding: 28px 32px 40px;
  }
  h2 {
    margin: 0 0 6px;
    font-size: 24px;
  }
  h3 {
    margin: 28px 0 4px;
    font-size: 16px;
  }
  .hint {
    margin: 0 0 12px;
    color: var(--muted);
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 16px;
  }
  .card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 22px 18px;
    border-radius: 14px;
    background: var(--raised);
    border: 1px solid var(--line);
    border-top: 4px solid var(--who);
  }
  .play {
    width: 96px;
    height: 96px;
    border-radius: 50%;
    font-size: 34px;
    border: 3px solid var(--who);
  }
  .play.on {
    background: var(--who);
    color: var(--on-accent);
  }
  .who {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--who);
  }
  .was {
    font-size: 15px;
    font-weight: 600;
  }
  .sub {
    font-size: 12px;
    color: var(--muted);
  }
  .name,
  .newname {
    font: inherit;
    color: inherit;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 6px 10px;
  }
  .name {
    width: 100%;
    box-sizing: border-box;
    font-size: 16px;
    text-align: center;
  }
  .tick {
    font-size: 13px;
    color: var(--muted);
  }
  .unsure {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .unsure li {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .unsure .on {
    border-color: var(--accent);
  }
  .now {
    color: var(--muted);
    flex: 1;
    min-width: 12em;
  }
  .end {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 28px;
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
