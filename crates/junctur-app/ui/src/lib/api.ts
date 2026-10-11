// The Rust side's commands and events (src/commands.rs, src/player.rs), typed.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface AudioTrack {
  id: number;
  title: string | null;
  lang: string | null;
  codec: string | null;
  channels: number | null;
  selected: boolean;
}

export interface PlayerState {
  path: string | null;
  position: number | null;
  duration: number | null;
  paused: boolean;
  audioTracks: AudioTrack[];
  hwdec: string | null;
  error: string | null;
}

export type StageState = "pending" | "running" | "done" | "failed";

export interface ProjectStatus {
  dir: string;
  stages: { name: string; state: StageState }[];
  transcript: boolean;
  edit: { ranges: number; kept: number; duration: number } | null;
  editError: string | null;
}

export interface Opened {
  project: ProjectStatus | null;
  projectError: string | null;
}

export const startupFile = () => invoke<string | null>("startup_file");
export const playerState = () => invoke<PlayerState>("player_state");
export const openFile = (path: string) => invoke<Opened>("open_file", { path });
export const togglePause = () => invoke<void>("toggle_pause");
export const seek = (seconds: number, exact: boolean) => invoke<void>("seek", { seconds, exact });
export const setAudioTrack = (id: number) => invoke<void>("set_audio_track", { id });

export const onPlayerState = (f: (s: PlayerState) => void): Promise<UnlistenFn> =>
  listen<PlayerState>("player-state", (e) => f(e.payload));
