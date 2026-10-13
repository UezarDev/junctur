<script lang="ts">
  // Portrait's mixer strips (2g, P4): beside a picture taller than wide, one strip per track with a
  // tall meter, Mute and Solo, its colour and name. The same controls as the lanes' headers (the
  // same features in both orientations); replaces 2e's row of strips over the play bar.
  import * as api from "./api";
  import { meter } from "./meters";
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

  async function call(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }
</script>

<div class="strips" role="group" aria-label="Audio tracks">
  {#each tracks as t (t.index)}
    <div class="strip" title={tr.title(t)}>
      <span class="meter" class:off={!tr.heard(t, tracks)}>
        <span class="bar" use:meter={{ track: t.index, axis: "y" }}></span>
      </span>
      <button class="ms m" class:on={t.muted} aria-pressed={t.muted} title="Mute (preview only)" onclick={() => call(() => api.setMuted(t.index, !t.muted))}
        >M</button
      >
      <button
        class="ms s"
        class:on={t.solo}
        aria-pressed={t.solo}
        title="Solo (preview only, not remembered)"
        onclick={() => call(() => api.setSolo(t.index, !t.solo))}>S</button
      >
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
  .ms {
    width: 22px;
    height: 20px;
    padding: 0;
    font-size: 10px;
    font-weight: 700;
    border-radius: 3px;
    color: var(--muted);
  }
  .m.on {
    color: #fff;
    background: rgba(239, 111, 94, 0.55);
    border-color: var(--bad);
  }
  .s.on {
    color: #101114;
    background: var(--accent);
    border-color: var(--accent);
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
