<script lang="ts">
  // Portrait's mixer strips (2g, P4): beside a picture taller than wide, one strip per track with a
  // tall meter, Mute and Solo, its colour and name. The same controls as the lanes' headers (the
  // same features in both orientations); replaces 2e's row of strips over the play bar.
  import type * as api from "./api";
  import { meter } from "./meters";
  import MuteSolo from "./MuteSolo.svelte";
  import * as tr from "./tracks";

  let {
    mixer,
    audio,
    speakers,
    onError,
  }: {
    mixer: api.MixerState | null;
    audio: api.AudioTrack[];
    speakers: api.Speaker[];
    onError: (message: string) => void;
  } = $props();

  const tracks = $derived(mixer?.tracks ?? []);

</script>

<div class="strips" role="group" aria-label="Audio tracks">
  {#each tracks as t (t.index)}
    <div class="strip" title={tr.title(t)}>
      <span class="meter" class:off={!tr.heard(t, tracks)}>
        <span class="bar" use:meter={{ track: t.index, axis: "y" }}></span>
      </span>
      <MuteSolo track={t} kind="mute" size="large" {onError} />
      <MuteSolo track={t} kind="solo" size="large" {onError} />
      <span class="dot" style:background={tr.colour(t, speakers)}></span>
      <span class="n">{t.index + 1}</span>
      <span class="name">{tr.name(t, audio)}</span>
    </div>
  {/each}
</div>

<style>
  .strips {
    display: flex;
    justify-content: center;
    gap: 14px;
    height: 100%;
    padding: 40px 10px 24px;
    box-sizing: border-box;
  }
  .strip {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    width: 52px;
    min-height: 0;
    font-size: 12px;
  }
  .meter {
    position: relative;
    flex: 1;
    width: 12px;
    min-height: 40px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }
  .bar {
    position: absolute;
    inset: 0;
    background: linear-gradient(to top, var(--good) 75%, var(--accent) 92%, var(--bad));
    transform: scaleY(0);
    transition: transform 80ms linear;
  }
  .meter.off .bar {
    background: var(--muted);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }
  .n {
    line-height: 1;
  }
  .name {
    max-width: 100%;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
