/**
 * How an audio track is named, described and coloured wherever it shows (2g: the lanes, the
 * portrait strips, Properties). Moved out of 2e's Mixer.svelte, which the lanes and strips replace.
 */
import type * as api from "./api";

const ROLE: Record<api.Role, string> = {
  mix: "Mix",
  voice: "Voice",
  game_pc: "Game",
  music: "Music",
  silent: "Silent",
  unknown: "Track",
};

/** Colours of tracks no speaker owns: the mix is the light one, everything else neutral. */
const MIX = "#d8d6d0";
const OTHER = "#8a8f99";

/** "Mix", the file's own title, "copy of 1", ... */
export function name(t: api.MixTrack, audio: api.AudioTrack[]): string {
  const title = audio.find((a) => a.id === t.index + 1)?.title;
  if (title) return title;
  if (t.copyOf !== null) return `copy of ${t.copyOf + 1}`;
  return t.role ? ROLE[t.role] : "Track";
}

/** The speakers told apart on this track (by track, 5a). */
export function speakersOn(t: api.MixTrack, speakers: api.Speaker[]): api.Speaker[] {
  return speakers.filter((s) => s.track === t.index);
}

/** The line under a lane's name: who is on it, else what it is. */
export function describe(t: api.MixTrack, audio: api.AudioTrack[], speakers: api.Speaker[]): string {
  const on = speakersOn(t, speakers);
  if (on.length) return on.map((s) => s.name).join(", ");
  if (t.copyOf !== null) return `same as track ${t.copyOf + 1}`;
  const channels = audio.find((a) => a.id === t.index + 1)?.channels;
  switch (t.role) {
    case "mix":
      return channels ? `all audio · ${channels}ch` : "all audio";
    case "voice":
      return "voice";
    case "game_pc":
      return "desktop audio";
    case "music":
      return "music";
    case "silent":
      return "silent";
    default:
      return t.role ? "not known" : "role not known yet";
  }
}

/** The track's colour: its speaker's caption colour when one speaker owns it, else neutral. */
export function colour(t: api.MixTrack, speakers: api.Speaker[]): string {
  const on = speakersOn(t, speakers);
  if (on.length) return on[0].color;
  return t.role === "mix" && t.copyOf === null ? MIX : OTHER;
}

/** Whether the track is heard now: with a solo anywhere, the soloed ones; else the unmuted. */
export function heard(t: api.MixTrack, all: api.MixTrack[]): boolean {
  return all.some((x) => x.solo) ? t.solo : !t.muted;
}

/** A hover text for a track's controls. */
export function title(t: api.MixTrack): string {
  const what = t.copyOf !== null ? `a copy of track ${t.copyOf + 1}` : t.role ? ROLE[t.role] : "role not known yet";
  return `Track ${t.index + 1}: ${what}`;
}

/** What an avatar shows: the first letter ("Uezar": U), with the number of a numbered name
 * ("Speaker 2": S2), so unnamed speakers stay apart. */
export function initials(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  const first = (words[0]?.[0] ?? "?").toUpperCase();
  const last = words[words.length - 1] ?? "";
  return words.length >= 2 && /^\d{1,2}$/.test(last) ? first + last : first;
}
