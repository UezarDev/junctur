<script lang="ts">
  // The speakers as avatars, lit while their turn is under the playhead. One component in three
  // sizes so the Speakers lane, the portrait side and the picture overlay agree on who is talking.
  import { onMount } from "svelte";
  import type * as api from "./api";
  import { share, subscribe } from "./meters";
  import { heard, initials } from "./tracks";

  let {
    speakers,
    talking,
    mixer,
    size,
  }: {
    speakers: api.Speaker[];
    talking: Set<number>;
    mixer: api.MixerState | null;
    size: "small" | "big" | "pill";
  } = $props();

  /** Bars per level readout, and how tall each is against the level (a fixed, uneven pattern). */
  const SHAPE = [0.55, 0.9, 0.7, 1, 0.6, 0.85, 0.5];
  const bars = new Map<number, HTMLElement[]>();

  function register(el: HTMLElement, id: number) {
    const list = bars.get(id) ?? [];
    list.push(el);
    bars.set(id, list);
    return {
      destroy() {
        const l = bars.get(id);
        if (l) bars.set(id, l.filter((x) => x !== el));
      },
    };
  }

  function levelOf(s: api.Speaker, levels: (number | null)[]): number {
    if (s.track != null) return share(levels[s.track]);
    const tracks = mixer?.tracks ?? [];
    let best = 0;
    for (const t of tracks) {
      if (heard(t, tracks)) best = Math.max(best, share(levels[t.index]));
    }
    return best;
  }

  onMount(() =>
    subscribe((levels) => {
      for (const s of speakers) {
        const list = bars.get(s.id);
        if (!list?.length) continue;
        const l = talking.has(s.id) ? levelOf(s, levels) : 0;
        list.forEach((el, i) => {
          el.style.transform = `scaleY(${Math.max(0.12, l * SHAPE[i % SHAPE.length])})`;
        });
      }
    }),
  );
</script>

<div class="avatars {size}">
  {#each speakers as s (s.id)}
    {@const on = talking.has(s.id)}
    <div class="who" class:on style:--c={s.color} title={on ? `${s.name} is talking` : s.name}>
      <span class="face">{initials(s.name)}</span>
      {#if size !== "small"}
        <span class="name">{s.name}</span>
        {#if on}
          <span class="level" aria-hidden="true">
            {#each SHAPE as _, i (i)}<span class="b" use:register={s.id}></span>{/each}
          </span>
        {:else}
          <span class="quiet" aria-hidden="true"></span>
        {/if}
      {/if}
    </div>
  {/each}
</div>

<style>
  .avatars {
    display: flex;
    gap: 4px;
  }
  .who {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
  .face {
    display: grid;
    place-items: center;
    border-radius: 50%;
    font-weight: 700;
    color: #101114;
    background: color-mix(in srgb, var(--c) 35%, #2a2b30);
    opacity: 0.75;
    transition:
      opacity 120ms,
      box-shadow 120ms;
  }
  .on .face {
    background: var(--c);
    opacity: 1;
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--c) 45%, transparent);
  }
  .level {
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
  }
  .b {
    display: block;
    width: 3px;
    background: var(--c);
    transform-origin: bottom;
    transform: scaleY(0.12);
    transition: transform 80ms linear;
  }
  .quiet {
    border-bottom: 2px dotted color-mix(in srgb, var(--c) 60%, transparent);
  }

  /* The Speakers lane's header. */
  .small .face {
    width: 18px;
    height: 18px;
    font-size: 9px;
  }

  /* Portrait's large avatars. */
  .big {
    flex-direction: column;
    align-items: center;
    gap: 18px;
  }
  .big .who {
    flex-direction: column;
    gap: 6px;
  }
  .big .face {
    width: 60px;
    height: 60px;
    font-size: 24px;
  }
  .big .on .face {
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--c) 50%, transparent);
  }
  .big .on .name {
    color: var(--text);
  }
  .big .level {
    height: 14px;
  }
  .big .b {
    height: 14px;
  }
  .big .quiet {
    width: 40px;
    height: 7px;
  }

  /* The overlay's pills on the picture. */
  .pill {
    flex-direction: column;
    gap: 6px;
  }
  .pill .who {
    padding: 3px 12px 3px 3px;
    border-radius: 999px;
    background: rgba(16, 17, 20, 0.78);
    border: 1px solid var(--line);
  }
  .pill .on {
    color: var(--text);
    border-color: color-mix(in srgb, var(--c) 70%, transparent);
  }
  .pill .face {
    width: 24px;
    height: 24px;
    font-size: 11px;
  }
  .pill .level {
    height: 12px;
  }
  .pill .b {
    height: 12px;
  }
  .pill .quiet {
    width: 26px;
    height: 6px;
  }
</style>
