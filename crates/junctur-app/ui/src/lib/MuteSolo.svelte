<script lang="ts">
  // A track's Mute or Solo button, shared by the lanes' headers and the portrait strips so both
  // behave alike: playback only; mutes are remembered per recording, solos are not.
  import * as api from "./api";

  let {
    track,
    kind,
    size = "small",
    onError,
  }: {
    track: api.MixTrack;
    kind: "mute" | "solo";
    size?: "small" | "large";
    onError: (message: string) => void;
  } = $props();

  const on = $derived(kind === "mute" ? track.muted : track.solo);
  const title = $derived(
    kind === "mute"
      ? on
        ? "Muted: click to hear it (preview only)"
        : "Mute (preview only)"
      : on
        ? "Soloed: click to unsolo"
        : "Solo: hear only the soloed tracks (preview only, not remembered)",
  );

  async function click() {
    try {
      await (kind === "mute" ? api.setMuted(track.index, !on) : api.setSolo(track.index, !on));
    } catch (e) {
      onError(String(e));
    }
  }
</script>

<button class="ms {kind} {size}" class:on aria-pressed={on} {title} onclick={click}>{kind === "mute" ? "M" : "S"}</button>

<style>
  .ms {
    flex: none;
    padding: 0;
    font-size: 10px;
    font-weight: 700;
    border-radius: 3px;
    color: var(--muted);
  }
  .small {
    width: 20px;
    height: 18px;
  }
  .large {
    width: 22px;
    height: 20px;
  }
  .mute.on {
    color: #fff;
    background: color-mix(in srgb, var(--bad) 55%, transparent);
    border-color: var(--bad);
  }
  .solo.on {
    color: var(--on-accent);
    background: var(--accent);
    border-color: var(--accent);
  }
</style>
