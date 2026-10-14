<script lang="ts">
  // The editor (2g): the video area, the play bar under it and the track lanes under that, the
  // side panel on the right (Transcript, Captions, Properties); in portrait, mixer strips and large
  // avatars beside the picture. Where everything goes comes from layout.ts; this places it, tells
  // mpv the margins, and holds what several parts share (the selection, the lanes' window, who is
  // talking). Above it all, the title bar (2i): its menus, the keys and test mode's `menu=<id>` all
  // come to `onMenu`; Settings opens over the editor or the start window.
  import { onMount, untrack } from "svelte";
  import { ask, open } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import * as api from "./lib/api";
  import Transport from "./lib/Transport.svelte";
  import PropertiesPanel from "./lib/PropertiesPanel.svelte";
  import StartWindow from "./lib/StartWindow.svelte";
  import TranscriptPanel from "./lib/TranscriptPanel.svelte";
  import CaptionsPanel from "./lib/CaptionsPanel.svelte";
  import CaptionsOverlay from "./lib/CaptionsOverlay.svelte";
  import Lanes from "./lib/Lanes.svelte";
  import Strips from "./lib/Strips.svelte";
  import Avatars from "./lib/Avatars.svelte";
  import ActionBar from "./lib/ActionBar.svelte";
  import Review from "./lib/Review.svelte";
  import TrackSteps from "./lib/TrackSteps.svelte";
  import * as steps from "./lib/steps";
  import TitleBar from "./lib/TitleBar.svelte";
  import SettingsDialog from "./lib/SettingsDialog.svelte";
  import { layout, TITLE } from "./lib/layout";
  import type { MenuId } from "./lib/menus";
  import { Talk, same } from "./lib/talking";
  import * as sel from "./lib/selection";
  import { words as toWords, type Words } from "./lib/transcript";

  // Raw (not deeply reactive): these are replaced whole on every update, and a 30 h transcript or
  // edit list read through a reactive proxy makes every layout pass several times slower.
  let player = $state.raw<api.PlayerState | null>(null);
  let opened = $state<api.Opened | null>(null);
  let edit = $state.raw<api.EditState | null>(null);
  // Every track's mute, solo and role (2e, 2g); the meters go straight to the bars (meters.ts).
  let mixer = $state.raw<api.MixerState | null>(null);
  // The open recording's analysis (2d): its stages' states replace the ones read at opening.
  let analysis = $state.raw<api.Progress | null>(null);
  // The transcription lane (2k): who transcribes and who waits, whichever recording is open, for
  // the Properties tab's job panel; each job's progress kept current from the analysis events.
  let lane = $state.raw<api.Transcription | null>(null);
  // The waveforms' read (2g); `wavesVersion` moves each time more of them is there to draw.
  let waves = $state.raw<api.WavesState | null>(null);
  let wavesVersion = $state(0);
  // Who talks when, and who is talking at the playhead.
  let talk = $state.raw(new Talk(null));
  let talking = $state.raw(new Set<number>());
  // The Properties tab shows the edit list as last read (it reloads when the file changes).
  const project = $derived.by(() => {
    const p = analysis?.project ?? opened?.project ?? null;
    return p && edit ? { ...p, edit: edit.summary, editError: edit.summary ? null : edit.error } : p;
  });
  // Set when the player could not start (e.g. libmpv missing) or a command failed.
  let failure = $state<string | null>(null);
  let dragging = $state(false);
  // Until the page knows whether the app was started with a file: the start window is not drawn
  // for a recording about to open (its rows would start a still and a probe each for nothing).
  let starting = $state(true);

  // The transcript and what is selected in it (or on the lanes or timeline).
  let words = $state.raw<Words | null>(null);
  let wordsLoading = $state(false);
  let selection = $state<sel.Selection | null>(null);
  // Review mode (2o): one clip at a time in the range toolbar's place; it takes the keys first.
  let reviewing = $state(false);
  let reviewer = $state<Review | null>(null);
  let panelOpen = $state(true);
  // Which side panel shows. The transcript stays mounted while hidden: laying out a 30 h
  // transcript again on every switch would cost a noticeable moment.
  let tab = $state<"transcript" | "captions" | "properties">("transcript");
  // The LV3 overlay: off by default, a viewer's preference kept in this page's storage.
  const OVERLAY_KEY = "junctur.talkingOverlay";
  let overlay = $state(readOverlay());
  let windowWidth = $state(window.innerWidth);
  let windowHeight = $state(window.innerHeight);
  let settingsOpen = $state(false);
  // Each open counts one up, and so does a close (2i): an open's answers that arrive after a close,
  // or after a newer open, are dropped. `closedAt` is the count a close left, so an open that lost
  // to a close (nothing opened since) closes again what the player opened meanwhile.
  let loadGen = 0;
  let closedAt = -1;
  let opening = $state(false);
  const PANEL = 400;
  const loaded = $derived(!!player?.path);
  const panelWidth = $derived(loaded && panelOpen ? Math.min(PANEL, windowWidth * 0.45) : 0);
  // A span per selected stretch (2p: words can be several), for the lanes and timeline to draw.
  const spans = $derived(sel.seconds(selection, words, edit?.fps ?? null));
  const speakers = $derived(edit?.speakers ?? []);
  // The layout from plain numbers: each is a derived value that only moves when it changes, so the
  // player's state (sent many times a second for the position) never lays the editor out again.
  const aspect = $derived(player?.aspect ?? null);
  const trackCount = $derived(mixer?.tracks.length ?? 0);
  const speakerCount = $derived(speakers.length);
  const L = $derived(
    layout({
      width: windowWidth,
      height: Math.max(0, windowHeight - TITLE),
      panel: panelWidth,
      aspect,
      tracks: trackCount,
      speakers: speakerCount,
    }),
  );
  // The layout's shares are of the window below the title bar; mpv's are of the whole window, the
  // bar being its top margin (the surface fills the window, under the opaque bar).
  const below = $derived(windowHeight > 0 ? Math.max(0, windowHeight - TITLE) / windowHeight : 1);
  const marginRight = $derived(loaded ? Math.round(L.margins.right * 1000) / 1000 : 0);
  const marginBottom = $derived(loaded ? Math.round(L.margins.bottom * below * 1000) / 1000 : 0);
  const marginTop = $derived(Math.round((1 - below) * 1000) / 1000);

  function readOverlay(): boolean {
    try {
      return localStorage.getItem(OVERLAY_KEY) === "on";
    } catch {
      return false;
    }
  }
  function setOverlay(on: boolean) {
    overlay = on;
    try {
      localStorage.setItem(OVERLAY_KEY, on ? "on" : "off");
    } catch {
      // A preference only: it is simply not kept.
    }
  }
  // Into review (on an editable list) or out of it; a selection would only sit under it unseen.
  function toggleReview() {
    if (reviewing) reviewing = false;
    // Not while a recording opens: review's first clip would play on the one coming.
    else if (edit?.editable && edit.fps && player?.path && !opening) {
      selection = null;
      reviewing = true;
    }
  }
  // The recording closed or its list stopped being editable (a re-analysis): review ends.
  $effect(() => {
    if (reviewing && (!player?.path || !edit?.editable)) reviewing = false;
  });

  function togglePanel() {
    panelOpen = !panelOpen;
  }

  // The side panel, play bar and lanes are opaque; mpv keeps the picture out from under them. Sent
  // once a window resize settles, and only when the shares really changed (each is a blocking mpv
  // call).
  let sent = "";
  let marginTimer = 0;
  $effect(() => {
    const r = marginRight;
    const b = marginBottom;
    const t = marginTop;
    clearTimeout(marginTimer);
    marginTimer = window.setTimeout(() => {
      const key = `${r} ${b} ${t}`;
      if (key === sent) return;
      sent = key;
      api.setVideoMargins(r, b, t).catch(() => (sent = ""));
    }, 100);
  });

  // The window the lanes show, in source seconds: three minutes (or the whole of a shorter
  // recording) until the user zooms.
  const DEFAULT_VIEW = 180;
  let view = $state.raw({ start: 0, len: DEFAULT_VIEW });
  const duration = $derived(player?.duration ?? 0);
  function setView(start: number, len: number) {
    const d = duration || len;
    const l = Math.min(d, Math.max(5, len));
    view = { start: Math.min(Math.max(0, start), Math.max(0, d - l)), len: l };
  }
  function zoom(by: number | "whole") {
    if (by === "whole") return setView(0, duration);
    const at = player?.position ?? view.start + view.len / 2;
    const len = view.len * by;
    setView(at - ((at - view.start) / view.len) * len, len);
  }
  // A new recording (or its length first known) starts on the default window.
  let viewFor = "";
  $effect(() => {
    const key = `${player?.path ?? ""}|${duration > 0}`;
    if (key === viewFor) return;
    viewFor = key;
    untrack(() => setView(0, DEFAULT_VIEW));
  });
  // The playhead leaving the window (playing on, or a seek elsewhere) pages the window to it.
  $effect(() => {
    const pos = player?.position;
    if (pos == null) return;
    untrack(() => {
      if (pos < view.start || pos > view.start + view.len) setView(pos - view.len * 0.05, view.len);
    });
  });

  // Who is talking at the playhead: replaced only when that changes, so the avatars do not
  // redraw on every position.
  $effect(() => {
    const now = talk.at(player?.position ?? -1);
    untrack(() => {
      if (!same(now, talking)) talking = now;
    });
  });

  // The speaker turns, fetched again when they change on disk (speakersKey), not on a rename.
  let turnsFor = "";
  $effect(() => {
    const key = `${player?.path ?? ""}|${edit?.speakersKey ?? ""}`;
    if (key === turnsFor) return;
    turnsFor = key;
    if (!player?.path) {
      talk = new Talk(null);
      return;
    }
    api.turns().then(
      (t) => {
        if (turnsFor === key) talk = new Talk(t);
      },
      (e) => (failure = String(e)),
    );
  });

  // The edit state's speakers and fixes keys the words were made with: when one moves (an outside
  // change to the turns, a word fixed here or by hand, 5h), the labels and text are fetched again.
  // A rename does not move them.
  let labelledWith = "";
  const wordsKey = () => `${edit?.speakersKey ?? ""}|${edit?.fixesKey ?? ""}`;

  async function loadTranscript() {
    growing++; // an ask from before this one is not taken (growTranscript)
    const gen = loadGen;
    words = null;
    selection = null;
    wordsLoading = true;
    try {
      const t0 = performance.now();
      labelledWith = wordsKey();
      const view = await api.transcript();
      const t1 = performance.now();
      if (gen !== loadGen) return;
      words = view ? toWords(view) : null;
      const n = words?.text.length ?? 0;
      void api.uiLog(
        `transcript ${n} words: fetched in ${(t1 - t0).toFixed(0)} ms, split in ${(performance.now() - t1).toFixed(0)} ms`,
      );
    } catch (e) {
      failure = String(e);
    } finally {
      wordsLoading = false;
    }
  }

  $effect(() => {
    const key = wordsKey();
    if (!words || wordsLoading || key === labelledWith) return;
    labelledWith = key;
    const before = words;
    api.transcript().then(
      (view) => {
        // Only onto the same transcript (another file may have opened meanwhile).
        if (view && words === before && view.speaker.length === before.text.length) {
          words = {
            ...before,
            text: view.text,
            speaker: Uint32Array.from(view.speaker),
            also: Uint32Array.from(view.also ?? []),
          };
        }
      },
      (e) => (failure = String(e)),
    );
  });

  const VIDEO = ["mp4", "mkv", "mov", "flv", "ts", "webm", "avi"];

  async function load(path: string) {
    const gen = ++loadGen;
    const stale = () => gen !== loadGen;
    opening = true;
    try {
      failure = null;
      const o = await api.openFile(path);
      if (stale()) {
        // Closed while opening: the player may have opened it after the close went through.
        if (closedAt === loadGen) void api.closeFile().catch(() => {});
        return;
      }
      opened = o;
      analysis = o.analysis;
      // Progress sent while the open was on its way was dropped (no `opened` yet to match it):
      // read where the run is now. A run that already ended needs nothing more, as the
      // transcript is read just below.
      const now = await api.analysis(o.file);
      if (stale()) return;
      // Never back: an event handled meanwhile may already be further on than this answer.
      if (now && opened === o && rank(now) >= rank(analysis)) {
        analysis = now;
        if (now.phase === "done") opened = { ...o, outdated: [] };
      }
      const w = await api.wavesState();
      if (stale()) return;
      if (w && w.file === o.file) {
        waves = w;
        wavesVersion++;
      }
      await loadTranscript();
    } catch (e) {
      if (!stale()) failure = String(e);
    } finally {
      if (!stale()) opening = false;
    }
  }

  const busy = steps.busy;
  const rank = steps.rank;

  // The words again while the recording transcribes (2j): a piece more of them, or the finished
  // transcript after the pieces. Only words at the end are new (their indices are the core's
  // promise), so the selection and what the panel shows stay; nothing is blanked meanwhile.
  // Asks in flight can answer out of order (a piece lands while the last ask is on its way): only
  // the latest ask's answer is taken, or a shorter list could replace a longer one.
  let growing = 0;
  async function growTranscript() {
    const file = opened?.file;
    const gen = loadGen;
    const ask = ++growing;
    try {
      const view = await api.transcript();
      if (opened?.file === file && gen === loadGen && ask === growing) words = view ? toWords(view) : null;
    } catch (e) {
      failure = String(e);
    }
  }

  function onProgress(p: api.Progress) {
    if (lane?.jobs.some((j) => j.file === p.file)) {
      lane = { ...lane, jobs: lane.jobs.map((j) => (j.file === p.file ? p : j)) };
    }
    if (!opened || p.file !== opened.file) return;
    const was = analysis;
    analysis = p;
    if (busy(was) && !busy(p)) {
      // The run ended: what the panel said was out of date was its business; the transcript may
      // be new (the edit list comes in through the watch). One that grew piece by piece is the
      // same words with more at the end.
      if (p.phase === "done") opened = { ...opened, outdated: [] };
      if (was?.transcribed != null && p.phase === "done") void growTranscript();
      else void loadTranscript();
    } else if (p.listed && !was?.listed) {
      // The run's list is written: from here the words are this run's, none until its first
      // piece, never an older run's transcript under indices the new words would reuse (2j).
      void loadTranscript();
    } else if (p.transcribed != null && p.transcribed !== was?.transcribed) {
      void growTranscript();
    }
  }

  // What the chip on the picture says while the open recording is analysed (mock L), with a thin
  // bar per track under it when there are several (2h).
  const chip = $derived(steps.label(analysis, steps.paused(lane, analysis?.file)));
  const chipSteps = $derived(steps.current(analysis));

  // Re-analyse: the pipeline with the settings now, in the background (5s). The user's edit list is
  // kept, with the speakers matched again against the remembered voices: nothing to ask. A fresh
  // cut (`fresh`) replaces their cut, so when the list holds their edits, ask first (it is kept as
  // a backup).
  async function reanalyse(fresh: boolean) {
    if (!opened) return;
    const file = opened.file;
    try {
      let r = await api.reanalyse(file, fresh, false);
      // Only a fresh cut is ever held back to ask; a plain Re-analyse never replaces the cut.
      if (fresh && r.handEdited) {
        const yes = await ask(
          "Your cut is replaced by a fresh one from the analysis; speaker names, fixed words and caption settings stay. Your version is kept beside it as edit.junctur.bak (or .bak2, …).",
          { title: "Start over with a fresh cut?", kind: "warning", okLabel: "Start over", cancelLabel: "Keep my cut" },
        );
        if (!yes) return;
        r = await api.reanalyse(file, true, true);
      }
      if (r.progress && opened?.file === file) analysis = r.progress;
    } catch (e) {
      failure = String(e);
    }
  }

  // Back to the start window (2i). The player stops and forgets the recording; what the page held
  // for it goes too. Edits were saved as they were made; an analysis of it goes on in the background.
  async function closeRecording() {
    // Also while a recording is still opening: the close wins (see `loadGen`).
    if (!player?.path && !opening) return;
    closedAt = ++loadGen;
    opening = false;
    try {
      await api.closeFile();
    } catch (e) {
      failure = String(e);
      return;
    }
    opened = null;
    analysis = null;
    words = null;
    selection = null;
    waves = null;
    wavesVersion++;
    failure = null;
  }

  // A model change surfaces as 2d's out-of-date notice for the open recording; nothing re-runs.
  function settingsSaved() {
    const o = opened;
    if (!o) return;
    api.outdated(o.file).then(
      (outdated) => {
        if (opened?.file === o.file) opened = { ...opened, outdated };
      },
      (e) => (failure = String(e)),
    );
  }

  async function pick() {
    try {
      const path = await open({ multiple: false, filters: [{ name: "Video", extensions: VIDEO }] });
      if (typeof path === "string") await load(path);
    } catch (e) {
      failure = String(e);
    }
  }

  async function run(f: () => Promise<void>) {
    try {
      await f();
    } catch (e) {
      failure = String(e);
    }
  }

  function onKey(e: KeyboardEvent) {
    // The dialog has the keys while it is open (Escape closes it).
    if (settingsOpen) return;
    // A field keeps every key, the title bar's shortcuts too (2i): Ctrl+W from a caption number
    // not yet committed would close the recording before its change is saved.
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
    if (reviewing && reviewer?.key(e)) {
      e.preventDefault();
      return;
    }
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && !e.shiftKey && (e.code === "KeyO" || e.code === "KeyW" || e.code === "Comma")) {
      e.preventDefault();
      return onMenu(e.code === "KeyO" ? "open" : e.code === "KeyW" ? "close" : "settings");
    }
    if (edit?.editable && player?.path) {
      if (ctrl && (e.code === "KeyY" || (e.code === "KeyZ" && e.shiftKey))) {
        e.preventDefault();
        return run(api.redo);
      }
      if (ctrl && e.code === "KeyZ") {
        e.preventDefault();
        return run(api.undo);
      }
      if (selection && (e.code === "Delete" || e.code === "Backspace")) {
        e.preventDefault();
        const s = selection;
        return run(() => sel.apply(s, false));
      }
      if (selection && e.code === "KeyK" && !ctrl) {
        const s = selection;
        return run(() => sel.apply(s, true));
      }
    }
    if (e.code === "Escape") {
      selection = null;
    } else if (e.code === "KeyT" && !ctrl && player?.path) {
      togglePanel();
    } else if (e.code === "Space" && player?.path) {
      e.preventDefault();
      run(api.togglePause);
    } else if (e.code === "KeyS" && !ctrl && player?.path && edit?.ranges.length) {
      run(() => api.setSkip(!edit?.skip));
    } else if (e.code === "KeyR" && !ctrl && !e.altKey) {
      toggleReview();
    }
  }

  // Every menu item, from the title bar, the keys or test mode (which sends any string).
  function onMenu(id: MenuId | string) {
    if (id === "open") void pick();
    else if (id === "close") void closeRecording();
    else if (id === "settings") settingsOpen = true;
    else if (id === "panel") {
      if (player?.path) togglePanel();
    } else if (id === "talking") setOverlay(!overlay);
    else if ((id === "transcript" || id === "captions" || id === "properties") && player?.path) {
      tab = id;
      if (!panelOpen) togglePanel();
    }
  }

  // Where the edit actions go (and review's bar, 2o): one fixed place, centred over the lanes' tracks just above their
  // bottom edge, with or without a selection. 2g put them at the selection, else at the playhead,
  // so the bar slid along while playing or scrubbing and was never where the hand left it (2n).
  const actionsAt = $derived.by(() => {
    const left = L.lanes.x + L.head + 1;
    const w = Math.max(1, L.lanes.w - L.head - 1);
    return { x: left + w / 2, min: left + 4, max: L.lanes.x + L.lanes.w - 4, bottom: windowHeight - TITLE - (L.lanes.y + L.lanes.h) + 8 };
  });

  onMount(() => {
    // A transcription started before the page (a reload) has sent no event yet; one that has wins.
    api.transcription().then((t) => (lane ??= t), () => {});
    api.playerState().then(
      async (s) => {
        player = s;
        edit = await api.editState();
        mixer = await api.mixer();
        const file = await api.startupFile();
        if (file) await load(file);
        starting = false;
      },
      (e) => {
        failure = String(e);
        starting = false;
      },
    );
    const unlisten = [
      api.onPlayerState((s) => (player = s)),
      api.onEditState((s) => (edit = s)),
      api.onMixer((s) => (mixer = s)),
      api.onAnalysis(onProgress),
      api.onTranscription((t) => (lane = t)),
      api.onWaves((w) => {
        if (opened && w.file === opened.file) {
          waves = w;
          wavesVersion++;
        }
      }),
      api.onMenu(onMenu),
      getCurrentWebview().onDragDropEvent((e) => {
        const p = e.payload;
        dragging = p.type === "enter" || p.type === "over";
        if (p.type === "drop" && p.paths.length > 0) load(p.paths[0]);
      }),
    ];
    return () => unlisten.forEach((u) => u.then((f) => f()));
  });
</script>

<svelte:window
  onkeydown={onKey}
  onresize={() => {
    windowWidth = window.innerWidth;
    windowHeight = window.innerHeight;
  }}
/>

<div class="frame">
<TitleBar
  menu={{ loaded, closable: loaded || opening, panel: panelOpen, talking: overlay, locked: settingsOpen }}
  {onMenu}
/>
<main class:dragging>
  {#if !player?.path}
    {#if !starting}
      <StartWindow ready={!!player} {dragging} onPick={pick} onOpen={load} onError={(e) => (failure = e)} />
    {/if}
  {:else}
    {#if L.strips}
      <div class="area" style:left="{L.strips.x}px" style:top="{L.strips.y}px" style:width="{L.strips.w}px" style:height="{L.strips.h}px">
        <Strips {mixer} audio={player.audioTracks} {speakers} onError={(e) => (failure = e)} />
      </div>
    {/if}
    {#if L.avatars}
      <div class="area talking" style:left="{L.avatars.x}px" style:top="{L.avatars.y}px" style:width="{L.avatars.w}px" style:height="{L.avatars.h}px">
        <h4>Talking now</h4>
        <Avatars {speakers} {talking} {mixer} size="big" />
      </div>
    {/if}
    <CaptionsOverlay {player} {edit} picture={L.picture} />
    {#if overlay && speakers.length}
      <div class="overlay" style:left="{L.picture.x + 14}px" style:top="{L.picture.y + 14}px">
        <Avatars {speakers} {talking} {mixer} size="pill" />
      </div>
    {/if}
    {#if chip}
      <div class="chip" style:right="{windowWidth - (L.picture.x + L.picture.w) + 14}px" style:top="{L.picture.y + 14}px">
        <span class="spin" aria-hidden="true"></span>
        <span class="what">
          <span>{chip}</span>
          {#if chipSteps.length > 1}<TrackSteps steps={chipSteps} compact />{/if}
        </span>
        <button
          onclick={() => {
            tab = "properties";
            if (!panelOpen) togglePanel();
          }}>Details</button
        >
      </div>
    {/if}
    <div class="box" style:top="{L.bar.y}px" style:width="{L.bar.w}px" style:height="{L.bar.h}px">
      <Transport
        {player}
        {edit}
        {spans}
        view={{ start: view.start, end: view.start + view.len }}
        onZoom={zoom}
        {reviewing}
        onReview={toggleReview}
        onSelect={(s) => (selection = s)}
        onError={(e) => (failure = e)}
      />
    </div>
    {#if L.lanes.h > 0}
      <div class="box" style:top="{L.lanes.y}px" style:width="{L.lanes.w}px" style:height="{L.lanes.h}px">
        <Lanes
          {player}
          {edit}
          {mixer}
          {talk}
          {talking}
          {view}
          row={L.row}
          speakersRow={L.speakersRow}
          head={L.head}
          thin={L.portrait}
          {wavesVersion}
          {spans}
          onView={setView}
          onSelect={(s) => (selection = s)}
          onError={(e) => (failure = e)}
        />
      </div>
    {/if}
    {#if reviewing}
      <Review bind:this={reviewer} {player} {edit} at={actionsAt} onDone={() => (reviewing = false)} onError={(e) => (failure = e)} />
    {:else}
      <ActionBar
        {player}
        {edit}
        {selection}
        wordsShown={panelWidth > 0 && tab === "transcript"}
        at={actionsAt} onSelect={(s) => (selection = s)} onError={(e) => (failure = e)} />
    {/if}
  {/if}

  {#if failure || player?.error}
    <div class="failure" role="alert" style:left="{L.video.w / 2}px">
      {failure ?? player?.error}
      <button class="x" onclick={() => (failure = null)} aria-label="Dismiss">×</button>
    </div>
  {/if}

  {#if panelWidth > 0}
    <div class="side" style:width="{panelWidth}px">
      <div class="tabs" role="tablist">
        <button role="tab" aria-selected={tab === "transcript"} class:on={tab === "transcript"} onclick={() => (tab = "transcript")}
          >Transcript</button
        >
        <button role="tab" aria-selected={tab === "captions"} class:on={tab === "captions"} onclick={() => (tab = "captions")}
          >Captions</button
        >
        <button role="tab" aria-selected={tab === "properties"} class:on={tab === "properties"} onclick={() => (tab = "properties")}
          >Properties</button
        >
      </div>
      <div class="pane" style:display={tab === "transcript" ? "block" : "none"}>
        <TranscriptPanel
          {words}
          loading={wordsLoading}
          analysing={busy(analysis)}
          listed={analysis?.listed ?? false}
          waiting={analysis?.phase === "waiting"}
          failed={analysis?.phase === "failed"}
          through={analysis?.listed && (busy(analysis) || analysis.phase === "failed") ? (analysis.transcribed ?? null) : null}
          {player}
          {edit}
          {selection}
          {reviewing}
          onSelect={(s) => (selection = s)}
          onError={(e) => (failure = e)}
        />
      </div>
      {#if tab === "captions"}
        <div class="pane">
          <CaptionsPanel {edit} source={player?.path ?? null} onError={(e) => (failure = e)} />
        </div>
      {:else if tab === "properties" && opened}
        <div class="pane">
          <PropertiesPanel
            {player}
            {edit}
            {mixer}
            status={project}
            error={opened.projectError}
            {analysis}
            {lane}
            outdated={opened.outdated}
            {waves}
            onReanalyse={() => reanalyse(false)}
            onFreshCut={() => reanalyse(true)}
            onError={(e) => (failure = e)}
          />
        </div>
      {/if}
    </div>
  {/if}

  {#if settingsOpen}
    <SettingsDialog onClose={() => (settingsOpen = false)} onSaved={settingsSaved} />
  {/if}
</main>
</div>

<style>
  .frame {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  main {
    position: relative;
    flex: 1;
    min-height: 0;
    box-sizing: border-box;
    outline: 2px dashed transparent;
    outline-offset: -10px;
    transition: outline-color 120ms;
  }
  main.dragging {
    outline-color: var(--accent);
  }
  .box,
  .area {
    position: absolute;
    left: 0;
  }
  .talking {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 14px;
  }
  .talking h4 {
    margin: 0;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--muted);
  }
  .overlay {
    position: absolute;
  }
  .chip {
    position: absolute;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 6px 5px 12px;
    border-radius: 8px;
    background: rgba(16, 17, 20, 0.86);
    border: 1px solid var(--line);
  }
  .chip .what {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 9em;
  }
  .chip button {
    padding: 2px 8px;
    color: var(--muted);
  }
  .spin {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid var(--accent);
    border-right-color: transparent;
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .side {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    display: flex;
    flex-direction: column;
    background: var(--panel-solid);
  }
  .tabs {
    display: flex;
    border-left: 1px solid var(--line);
    border-bottom: 1px solid var(--line);
  }
  .tabs button {
    flex: 1;
    padding: 8px 0;
    color: var(--muted);
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
  }
  .tabs button.on {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .pane {
    flex: 1;
    min-height: 0;
  }
  .failure {
    position: absolute;
    z-index: 10;
    transform: translateX(-50%);
    top: 16px;
    max-width: 60%;
    padding: 8px 30px 8px 12px;
    border-radius: 6px;
    background: var(--panel);
    border: 1px solid var(--bad);
    color: var(--bad);
  }
  .failure .x {
    position: absolute;
    top: 4px;
    right: 4px;
    padding: 0 6px;
    border: none;
    background: none;
    color: var(--muted);
  }
</style>
