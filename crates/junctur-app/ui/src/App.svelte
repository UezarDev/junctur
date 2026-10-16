<script lang="ts">
  // The editor: video area, play bar, lanes and side panel, placed from layout.ts, plus what several
  // parts share (the selection, the lanes' window, who is talking). Every menu ends in `onMenu`.
  import { onMount, untrack } from "svelte";
  import { ask as ask_, open } from "@tauri-apps/plugin-dialog";
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
  import ReanalyseDialog from "./lib/ReanalyseDialog.svelte";
  import MomentsPanel from "./lib/MomentsPanel.svelte";
  import LikePanel from "./lib/LikePanel.svelte";
  import AskPanel from "./lib/AskPanel.svelte";
  import * as tr from "./lib/tracks";
  import RollCall from "./lib/RollCall.svelte";
  import { floatAt, layout, REVIEW, TITLE, TOOLS } from "./lib/layout";
  import type { MenuId } from "./lib/menus";
  import { Talk, same } from "./lib/talking";
  import * as sel from "./lib/selection";
  import { words as toWords, type Words } from "./lib/transcript";
  import { blockReload } from "./lib/noReload";
  import { clips, holding, type Clip } from "./lib/review";
  import { toFrame } from "./lib/time";

  // Raw (not deeply reactive): these are replaced whole on every update, and a 30 h transcript or
  // edit list read through a reactive proxy makes every layout pass several times slower.
  let player = $state.raw<api.PlayerState | null>(null);
  // Raw: replaced whole, never changed in place, and `attach` compares it with the object it set;
  // a deep `$state` would store a proxy, so that check would never hold.
  let opened = $state.raw<api.Opened | null>(null);
  // The open recording's labels for its silent parts (the Labels lane); null when it has none.
  let labels = $state.raw<api.Labels | null>(null);
  // The open recording's smart keep: its prompt and suggested keeps (the Suggestions lane).
  let keep = $state.raw<api.KeepView | null>(null);
  // The open recording's marked moments (the Moments tab and lane, 7a); the one last clicked on the lane.
  let moments = $state.raw<api.MomentsView | null>(null);
  let momentFocus = $state<number | null>(null);
  // The open recording's "find all like this" search (the Like this tab and lane, 7b); the match
  // last clicked on the lane, and whether a search is running.
  let like = $state.raw<api.LikeView | null>(null);
  let likeFocus = $state<number | null>(null);
  let likeSearching = $state(false);
  // The open recording's typed requests (the Ask tab and lane, 7c); the ghost last clicked on the
  // lane, and whether a request is being read.
  let asked = $state.raw<api.AskView | null>(null);
  let askFocus = $state<{ step: number; index: number } | null>(null);
  let asking = $state(false);
  // The speaker roll call over the editor (7a); it opens by itself once per recording.
  let rollCallOpen = $state(false);
  let rollCallAsked = "";
  let momentsShownFor = "";
  // Review over only these clips (the uncertain suggestions), or over every clip when null.
  let reviewOnly = $state.raw<Clip[] | null>(null);
  let edit = $state.raw<api.EditState | null>(null);
  // Every track's mute, solo and role; the meters go straight to the bars (meters.ts).
  let mixer = $state.raw<api.MixerState | null>(null);
  // The open recording's analysis: its stages' states replace the ones read at opening.
  let analysis = $state.raw<api.Progress | null>(null);
  // The transcription lane: who transcribes and who waits, whichever recording is open, for
  // the Properties tab's job panel; each job's progress kept current from the analysis events.
  let lane = $state.raw<api.Transcription | null>(null);
  // The waveforms' read; `wavesVersion` moves each time more of them is there to draw.
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
  // Review mode: one clip at a time in the range toolbar's corner; it takes the keys first.
  let reviewing = $state(false);
  let reviewer = $state<Review | null>(null);
  let panelOpen = $state(true);
  // Which side panel shows. The transcript stays mounted while hidden: laying out a 30 h
  // transcript again on every switch would cost a noticeable moment.
  let tab = $state<"transcript" | "captions" | "properties" | "moments" | "like" | "ask">("transcript");
  // The talking overlay: off by default, a viewer's preference kept in this page's storage.
  const OVERLAY_KEY = "junctur.talkingOverlay";
  let overlay = $state(readOverlay());
  let windowWidth = $state(window.innerWidth);
  let windowHeight = $state(window.innerHeight);
  let settingsOpen = $state(false);
  // The transcript flags words Whisper was less sure of than this (Settings); none until read.
  // The default is the core's `config::FLAG_BELOW`, sent with the settings; not repeated here.
  let flagBelow = $state(0);
  // Each open counts one up, and so does a close: an open's answers that arrive after a close,
  // or after a newer open, are dropped. `closedAt` is the count a close left, so an open that lost
  // to a close (nothing opened since) closes again what the player opened meanwhile.
  let loadGen = 0;
  let closedAt = -1;
  let opening = $state(false);
  const PANEL = 400;
  const loaded = $derived(!!player?.path);
  const panelWidth = $derived(loaded && panelOpen ? Math.min(PANEL, windowWidth * 0.45) : 0);
  // A span per selected stretch (words can be several), for the lanes and timeline to draw.
  const spans = $derived(sel.seconds(selection, words, edit?.fps ?? null));
  // The selected stretch's own word edges, which the lanes let be dragged. Read from plain numbers,
  // so the player's state, sent many times a second, does not make them again.
  const duration = $derived(player?.duration ?? 0);
  // While transcribing: every word before this many seconds is in.
  const through = $derived(
    analysis?.listed && (steps.busy(analysis) || analysis.phase === "failed") ? (analysis.transcribed ?? null) : null,
  );
  const wordEdges = $derived(sel.wordEdges(selection, words, edit?.fps ?? null, duration, through));
  const speakers = $derived(edit?.speakers ?? []);
  // The layout from plain numbers: each is a derived value that only moves when it changes, so the
  // player's state (sent many times a second for the position) never lays the editor out again.
  const aspect = $derived(player?.aspect ?? null);
  const trackCount = $derived(mixer?.tracks.length ?? 0);
  const speakerCount = $derived(speakers.length);
  const hasLabels = $derived((labels?.pieces.length ?? 0) > 0);
  const hasKeep = $derived(!!keep?.judgedPrompt);
  const hasMoments = $derived((moments?.moments ?? []).some((m) => m.state !== "rejected"));
  const hasLike = $derived(!!like && (like.examples.length > 0 || like.matches.length > 0));
  const hasAsk = $derived(!!asked && asked.steps.length > 0);
  const likeTracks = $derived((mixer?.tracks ?? []).map((t) => ({ index: t.index, name: tr.name(t, player?.audioTracks ?? []) })));
  const L = $derived(
    layout({
      width: windowWidth,
      height: Math.max(0, windowHeight - TITLE),
      panel: panelWidth,
      aspect,
      tracks: trackCount,
      speakers: speakerCount,
      labels: hasLabels,
      keep: hasKeep,
      moments: hasMoments,
      like: hasLike,
      ask: hasAsk,
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
  function toggleReview(only: Clip[] | null = null) {
    if (reviewing) reviewing = false;
    // Not while a recording opens: review's first clip would play on the one coming.
    else if (edit?.editable && edit.fps && player?.path && !opening) {
      selection = null;
      reviewOnly = only;
      reviewing = true;
    }
  }
  // Review over smart keep's uncertain suggestions only, each a clip as the list has it now.
  function reviewUncertain() {
    const fps = edit?.fps;
    if (!fps || !keep || !edit) return;
    // A kept clip carries the number of the kept range it is in, as review and the toolbar name it.
    const all = clips(edit);
    const range = (frame: number) => {
      const c = all[holding(all, frame)];
      return c?.kept ? c.index : 0;
    };
    const only = keep.items
      .filter((i) => i.state === "suggested" && i.uncertain)
      .map((i) => ({
        start: toFrame(i.start, fps, Math.floor),
        end: toFrame(i.end, fps, Math.ceil),
        kept: !i.keep,
        index: i.keep ? 0 : range(toFrame(i.start, fps, Math.floor)),
      }))
      .filter((c) => c.end > c.start);
    // Review keeps the list it started with: one already open goes on with its own.
    if (only.length && !reviewing) toggleReview(only);
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
  // change to the turns, a word fixed here or by hand), the labels and text are fetched again.
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

  // Each refetch's number: the latest one asked for wins, so of two quick edge moves the later one's
  // words are the ones kept.
  let relabel = 0;
  $effect(() => {
    const key = wordsKey();
    if (!words || wordsLoading || key === labelledWith) return;
    labelledWith = key;
    const ask = ++relabel;
    const gen = loadGen;
    api.transcript().then(
      (view) => {
        // Only the latest ask, onto the same recording's transcript (another file may have opened
        // meanwhile) and the same words.
        if (view && ask === relabel && gen === loadGen && words && view.speaker.length === words.text.length) {
          // Text, speakers and the spans fixed as one text all come from the view.
          words = toWords(view);
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
      await attach(o, stale);
    } catch (e) {
      if (!stale()) failure = String(e);
    } finally {
      if (!stale()) opening = false;
    }
  }

  // A page that started over while the app went on (a dev build's Vite, a crash of the page): the
  // player has a recording open, and the page takes it as it is from the backend, not reopening it.
  async function reattach() {
    const gen = ++loadGen;
    const stale = () => gen !== loadGen;
    opening = true;
    try {
      // This page is not in review, so neither is the player (its loop, the skip toggle it holds).
      await api.endReview().catch(() => {});
      const o = await api.currentRecording();
      if (stale() || !o) return;
      await attach(o, stale);
      void api.uiLog(`page reattached to ${o.file}: ${words?.text.length ?? 0} words, analysis ${analysis?.phase ?? "none"}`);
    } catch (e) {
      if (!stale()) failure = String(e);
    } finally {
      if (!stale()) opening = false;
    }
  }

  // What the page holds for an open recording, from what opening it (or `currentRecording`) said:
  // its analysis as it is now, its waveforms, its transcript.
  async function attach(o: api.Opened, stale: () => boolean) {
    opened = o;
    // The last recording's moments would answer for this one's until its own are read.
    moments = null;
    momentFocus = null;
    like = null;
    likeFocus = null;
    asked = null;
    askFocus = null;
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
    void loadLabels();
    void loadKeep();
    void loadMoments();
    void loadLike();
    void loadAsk();
    const w = await api.wavesState();
    if (stale()) return;
    if (w && w.file === o.file) {
      waves = w;
      wavesVersion++;
    }
    await loadTranscript();
  }

  const busy = steps.busy;
  const rank = steps.rank;

  // The labels of the open recording as its project has them now; only the latest ask's answer for
  // the recording still open is taken.
  let labelsAsk = 0;
  async function loadLabels() {
    const o = opened;
    const ask = ++labelsAsk;
    if (!o) {
      labels = null;
      return;
    }
    try {
      const l = await api.labels(o.file);
      if (ask === labelsAsk && opened?.file === o.file) {
        labels = l;
        void api.uiLog(`labels: ${l?.pieces.length ?? 0} pieces`);
      }
    } catch (e) {
      // The lane is an extra: a labels file that will not read is logged, not a banner over the editor.
      void api.uiLog(`labels could not be read: ${e}`);
    }
  }
  // Smart keep of the open recording as its project has it now; the latest ask's answer only.
  let keepAsk = 0;
  async function loadKeep() {
    const o = opened;
    const ask = ++keepAsk;
    if (!o) {
      keep = null;
      return;
    }
    try {
      const k = await api.keepView(o.file);
      if (ask === keepAsk && opened?.file === o.file) {
        keep = k;
        void api.uiLog(`keep: ${k.items.length} suggestions, judged ${k.judgedPrompt ? "yes" : "no"}`);
      }
    } catch (e) {
      // An extra, like the labels: logged, not a banner over the editor.
      void api.uiLog(`smart keep could not be read: ${e}`);
    }
  }
  // After an edit the suggestions move (a hand edit wins over one) and the pairs are collected
  // again: read once the edits settle.
  let keepTimer = 0;
  $effect(() => {
    void edit;
    if (!untrack(() => keep?.judgedPrompt)) return;
    clearTimeout(keepTimer);
    keepTimer = window.setTimeout(() => void loadKeep(), 500);
  });
  // The moments of the open recording as its project has them now; the latest ask's answer only. The
  // first time a recording shows waiting moments, the side panel opens on them (7a: the editor opens
  // on a list of moments, not on a long timeline).
  let momentsAsk = 0;
  async function loadMoments() {
    const o = opened;
    const ask = ++momentsAsk;
    if (!o) {
      moments = null;
      return;
    }
    try {
      const m = await api.momentsView(o.file);
      if (ask !== momentsAsk || opened?.file !== o.file) return;
      moments = m;
      void api.uiLog(`moments: ${m.moments.length}, ${m.moments.filter((x) => x.state === "suggested").length} waiting`);
      if (momentsShownFor !== o.file && m.moments.some((x) => x.state === "suggested")) {
        momentsShownFor = o.file;
        tab = "moments";
        if (!panelOpen) togglePanel();
      }
    } catch (e) {
      void api.uiLog(`moments could not be read: ${e}`);
    }
  }
  // Edits move what an answer may do, and words coming in may hold new phrases: read once they settle.
  let momentsTimer = 0;
  $effect(() => {
    void edit;
    void analysis?.transcribed;
    clearTimeout(momentsTimer);
    momentsTimer = window.setTimeout(() => void loadMoments(), 700);
  });
  async function momentAnswer(action: api.MomentAction, m: api.Moment | null) {
    try {
      await api.momentAnswer(action, m?.at);
    } catch (e) {
      failure = String(e);
    }
    void loadMoments();
  }
  // Jumps to a moment's start and plays it.
  async function jumpTo(m: api.Moment) {
    momentFocus = m.at;
    try {
      await api.playFrom(m.start);
    } catch (e) {
      failure = String(e);
    }
  }
  function momentClicked(m: api.Moment) {
    tab = "moments";
    if (!panelOpen) togglePanel();
    void jumpTo(m);
  }
  // The like-this search of the open recording; the latest ask's answer only.
  let likeAsk = 0;
  async function loadLike() {
    const o = opened;
    const ask = ++likeAsk;
    if (!o) {
      like = null;
      return;
    }
    try {
      const v = await api.likeView(o.file);
      if (ask !== likeAsk || opened?.file !== o.file) return;
      like = v;
    } catch (e) {
      void api.uiLog(`like this could not be read: ${e}`);
      failure = `The Like this search could not be read: ${e}`;
    }
  }
  // The page's settings show at once (the lane and slider move live) and are saved once they
  // settle; a search, act or answer saves what is waiting first.
  let likeSave: { file: string; s: api.LikeSettings } | null = null;
  let likeTimer = 0;
  function likeSettings(s: api.LikeSettings) {
    const o = opened;
    if (!o || !like) return;
    likeAsk++;
    // Another track or range: the matches are another search's (the engine drops them too).
    const other = s.track !== like.track || JSON.stringify(s.range) !== JSON.stringify(like.range);
    const gone = other ? { matches: [], searched: null, proposal: null } : {};
    like = { ...like, ...s, ...gone, effectiveTrack: s.track ?? like.gameTrack ?? 0 };
    likeSave = { file: o.file, s };
    clearTimeout(likeTimer);
    likeTimer = window.setTimeout(() => void flushLike(), 400);
  }
  async function flushLike() {
    clearTimeout(likeTimer);
    const w = likeSave;
    likeSave = null;
    if (!w) return;
    try {
      const v = await api.likeSet(w.file, w.s);
      // Only when nothing newer is waiting: the page's own copy is ahead.
      if (!likeSave && opened?.file === w.file) like = v;
    } catch (e) {
      failure = String(e);
    }
  }
  async function likeRun(f: (file: string) => Promise<api.LikeView>) {
    const o = opened;
    if (!o) return;
    await flushLike();
    likeAsk++;
    try {
      const v = await f(o.file);
      if (opened?.file !== o.file) return;
      // Settings changed while it ran are the page's newest: they stay over the answer's (their
      // own timer saves them).
      const w = likeSave;
      like = w ? { ...v, ...w.s, effectiveTrack: w.s.track ?? v.gameTrack ?? 0 } : v;
    } catch (e) {
      failure = String(e);
    }
  }
  async function likeSearch() {
    likeSearching = true;
    try {
      await likeRun((file) => api.likeSearch(file));
      if (like) void api.uiLog(`like this: ${like.matches.length} found in ${like.searched?.seconds ?? "?"} s`);
    } finally {
      likeSearching = false;
    }
  }
  const likeAct = (act: api.LikeAct) => likeRun((file) => api.likeAct(file, act)).then(() => void loadMoments());
  const likeAnswer = (answer: api.LikeAnswer, id?: number) =>
    likeRun(() => api.likeAnswer(answer, id)).then(() => void loadMoments());
  // Plays a match or an example with a little either side.
  async function likePlay(span: { start: number; end: number }) {
    try {
      await api.playBetween(Math.max(0, span.start - 0.75), span.end + 0.75);
    } catch (e) {
      failure = String(e);
    }
  }
  function likeClicked(m: api.LikeMatch) {
    likeFocus = m.point;
    tab = "like";
    if (!panelOpen) togglePanel();
    void likePlay(m);
  }

  // The typed requests of the open recording; the latest ask's answer only.
  let askSeq = 0;
  async function loadAsk() {
    const o = opened;
    const n = ++askSeq;
    if (!o) {
      asked = null;
      return;
    }
    try {
      const v = await api.askView(o.file);
      if (n !== askSeq || opened?.file !== o.file) return;
      asked = v;
    } catch (e) {
      void api.uiLog(`the requests could not be read: ${e}`);
      failure = `The Ask tab's requests could not be read: ${e}`;
    }
  }
  async function askRun(f: (file: string) => Promise<api.AskView>) {
    const o = opened;
    if (!o) return;
    askSeq++;
    try {
      const v = await f(o.file);
      if (opened?.file === o.file) asked = v;
    } catch (e) {
      failure = String(e);
      void loadAsk();
    }
  }
  async function askRequest(request: string, range: [number, number] | null, model: api.AskSize | null) {
    asking = true;
    try {
      await askRun((file) => api.askRequest(file, request, range, model));
      // A preview playing shows the new ghosts too.
      if (edit?.preview) await askPreview(true);
      const s = asked?.steps[asked.steps.length - 1];
      if (s)
        void api.uiLog(
          `ask: "${s.request}" read by ${s.reading.reader} in ${s.reading.answerS ?? "?"} s: ${s.described || s.error} (${s.ghosts.length} ghosts)`,
        );
    } finally {
      asking = false;
    }
  }
  const askRerun = (id: number, recipe: api.Recipe) =>
    askRun((file) => api.askRerun(file, id, recipe)).then(() => (edit?.preview ? askPreview(true) : undefined));
  const askAnswer = (answer: api.AskAnswer, id: number, which?: number[], applied?: number) =>
    askRun(() => api.askAnswer(answer, id, which, applied)).then(() => void loadMoments());
  async function askPreview(on: boolean) {
    try {
      await api.askPreview(on);
    } catch (e) {
      failure = String(e);
    }
  }
  async function askPlay(at: number) {
    try {
      await api.playFrom(at);
    } catch (e) {
      failure = String(e);
    }
  }
  function askClicked(p: { step: api.AskStep; index: number; ghost: api.AskGhost }) {
    askFocus = { step: p.step.id, index: p.index };
    tab = "ask";
    if (!panelOpen) togglePanel();
    void askPlay(Math.max(0, p.ghost.start - 1.5));
  }

  // The roll call opens by itself once, the first time a recording's list is editable with speakers
  // in it, unless it was shown for that recording before.
  $effect(() => {
    const o = opened;
    if (!o || !edit?.editable || !edit.speakers.length || rollCallOpen || reviewing) return;
    if (rollCallAsked === o.file) return;
    rollCallAsked = o.file;
    api.rollCall(o.file).then(
      (v) => {
        if (!v.seen && v.cards.length && opened?.file === o.file) rollCallOpen = true;
      },
      (e) => void api.uiLog(`roll call could not be read: ${e}`),
    );
  });
  function rollCallClosed(lines: string[]) {
    rollCallOpen = false;
    for (const l of lines) void api.uiLog(`roll call: ${l}`);
  }

  async function keepAnswer(action: api.KeepAction, item: api.KeepItem) {
    try {
      await api.keepAnswer(action, item.start, item.end);
    } catch (e) {
      failure = String(e);
    }
    void loadKeep();
  }

  /** Whether `p`'s labels stage is done (made, reused or off). */
  const labelled = (p: api.Progress | null) =>
    p?.project?.stages.some((s) => s.name === "labels" && s.state === "done") ?? false;

  // The words again while the recording transcribes: a piece more, or the finished transcript. Only
  // words at the end are new (the core's promise), so the selection and the panel stay. Only the
  // latest ask's answer is taken, or a shorter list could replace a longer one.
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
    // The labels stage done during the run (they show while Whisper runs), or the run over.
    if ((labelled(p) && !labelled(was)) || (busy(was) && !busy(p))) void loadLabels();
    // The run judged at its end (smart keep on, or a prompt): the suggestions are new.
    if (busy(was) && !busy(p)) void loadKeep();
    if (busy(was) && !busy(p)) void loadMoments();
    if (busy(was) && !busy(p)) void loadLike();
    if (busy(was) && !busy(p)) void loadAsk();
    if (busy(was) && !busy(p)) {
      // The run ended: what the panel said was out of date was its business; the transcript may
      // be new (the edit list comes in through the watch). One that grew piece by piece is the
      // same words with more at the end.
      if (p.phase === "done") opened = { ...opened, outdated: [] };
      if (was?.transcribed != null && p.phase === "done") void growTranscript();
      else void loadTranscript();
    } else if (p.listed && !was?.listed) {
      // The run's list is written: from here the words are this run's, none until its first
      // piece, never an older run's transcript under indices the new words would reuse.
      void loadTranscript();
    } else if (p.transcribed != null && p.transcribed !== was?.transcribed) {
      void growTranscript();
    }
  }

  // What the chip on the picture says while the open recording is analysed, with a thin bar per
  // track under it when there are several.
  const chip = $derived(steps.label(analysis, steps.paused(lane, analysis?.file)));
  const chipSteps = $derived(steps.current(analysis));

  // The Correct pop-up (ReanalyseDialog): open while `correcting` holds the word bar's waiting
  // promise, answered with what started (for its note) or null.
  let correcting = $state<{ done: (r: Corrected | null) => void } | null>(null);
  // What the word bar's note says a run is doing, with the run's answer.
  type Corrected = { r: api.Corrected; doing: string };
  // The selection as the pop-up takes it: one stretch from its first second to its last (several
  // stretches of words count as the span holding them all).
  const reanalyseSpan = $derived.by(() => {
    if (!spans.length) return null;
    const start = Math.min(...spans.map((s) => s.start));
    const end = Math.max(...spans.map((s) => s.end));
    return end > start ? { start, end, what: sel.describe(selection, edit?.fps ?? null) } : null;
  });

  // The word bar's (and the empty transcript's) Correct: opens the pop-up; resolves once it starts a
  // run or is closed.
  function correct(): Promise<Corrected | null> {
    if (!opened) return Promise.resolve(null);
    correcting?.done(null);
    return new Promise((done) => (correcting = { done }));
  }
  function closeCorrect(r: Corrected | null = null) {
    correcting?.done(r);
    correcting = null;
  }

  // What a run asked for does, in a few words for the word bar's note.
  function doing(ask: api.Ask): string {
    const parts = [
      ask.words ? "transcribing the words again" : "",
      ask.speakers ? (ask.keepNames ? "hearing the speakers again" : "hearing the speakers again and naming them afresh") : "",
      ask.keepCuts ? "" : "making a fresh cut",
      ask.keepFixes ? "" : "dropping your fixed words",
    ].filter((p) => p);
    const what = parts.length ? parts.join(", ") : "running what the settings changed";
    return ask.within ? `${what} in the selection` : what;
  }

  // Runs what the pop-up asks. A fresh cut over the user's edits is asked about first (the cut
  // is theirs); declined, the pop-up stays for another choice.
  async function reanalyse(
    choice: Omit<api.Ask, "within" | "replace">,
    within: [number, number] | null,
  ): Promise<{ error: string | null; started: boolean }> {
    if (!opened) return { error: null, started: false };
    const file = opened.file;
    const ask: api.Ask = { ...choice, within, replace: false };
    try {
      let r = await api.reanalyse(file, ask);
      if (r.handEdited) {
        const yes = await ask_(
          within
            ? "Your cut in the selection is replaced by a fresh one from the analysis; the rest of your cut stays. Your version is kept beside it as edit.junctur.bak (or .bak2, …)."
            : "Your cut is replaced by a fresh one from the analysis. Your version is kept beside it as edit.junctur.bak (or .bak2, …).",
          { title: "Make a fresh cut?", kind: "warning", okLabel: "Make a fresh cut", cancelLabel: "Keep my cut" },
        );
        if (!yes || opened?.file !== file) return { error: null, started: false };
        r = await api.reanalyse(file, { ...ask, replace: true });
      }
      if (r.progress && opened?.file === file) analysis = r.progress;
      if (r.progress) closeCorrect({ r, doing: doing(ask) });
      return { error: null, started: r.progress != null };
    } catch (e) {
      return { error: String(e), started: false };
    }
  }

  // Back to the start window. The player stops and forgets the recording; what the page held
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
    labels = null;
    keep = null;
    moments = null;
    like = null;
    asked = null;
    rollCallOpen = false;
    words = null;
    selection = null;
    waves = null;
    wavesVersion++;
    failure = null;
  }

  // A model change surfaces as the out-of-date notice for the open recording; nothing re-runs.
  function settingsSaved(v: api.SettingsView) {
    flagBelow = v.settings.flagBelow;
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
    // A dialog has the keys while it is open (Escape closes it).
    if (settingsOpen || correcting || rollCallOpen) return;
    // A field, a textarea or anything editable keeps every key, the title bar's shortcuts too:
    // Ctrl+W from an uncommitted number would close before saving, Backspace would cut the lanes.
    const t = e.target;
    if (t instanceof HTMLInputElement || t instanceof HTMLSelectElement || t instanceof HTMLTextAreaElement) return;
    if (t instanceof HTMLElement && t.isContentEditable) return;
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
    else if ((id === "transcript" || id === "captions" || id === "properties" || id === "moments") && player?.path) {
      tab = id;
      if (!panelOpen) togglePanel();
    } else if (id === "rollcall" && opened) rollCallOpen = true;
  }

  // Where the edit actions and review's bar go: the picture's bottom-right corner, risen above the
  // caption block when the captions preview would sit under them (layout.ts `floatAt`).
  const band = $derived(edit?.editable && edit.captions?.preview ? { ...edit.captions, speakers: edit.speakers.length } : null);
  const toolsAt = $derived(floatAt(L.picture, L.video, TOOLS, band));
  const reviewAt = $derived(floatAt(L.picture, L.video, REVIEW, band));

  onMount(() => {
    // A transcription started before the page (a reload) has sent no event yet; one that has wins.
    api.transcription().then((t) => (lane ??= t), () => {});
    api.settings().then(
      (v) => (flagBelow = v.settings.flagBelow),
      () => {},
    );
    api.playerState().then(
      async (s) => {
        player = s;
        edit = await api.editState();
        mixer = await api.mixer();
        const file = await api.startupFile();
        if (file) await load(file);
        else if (s.path) await reattach();
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
      api.onKeep((r) => {
        if (opened && r.file === opened.file) {
          if (keep) keep = { ...keep, run: r };
          if (!r.running) void loadKeep();
        }
      }),
      api.onWaves((w) => {
        if (opened && w.file === opened.file) {
          waves = w;
          wavesVersion++;
        }
      }),
      api.onMenu(onMenu),
      Promise.resolve(blockReload()),
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
  menu={{ loaded, closable: loaded || opening, panel: panelOpen, talking: overlay, locked: settingsOpen || correcting != null }}
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
        onReview={() => toggleReview()}
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
          labelsRow={L.labelsRow}
          {labels}
          keepRow={L.keepRow}
          keepItems={keep?.items ?? []}
          onKeep={keepAnswer}
          momentsRow={L.momentsRow}
          moments={moments?.moments ?? []}
          onMoment={momentClicked}
          likeRow={L.likeRow}
          {like}
          onLike={likeClicked}
          askRow={L.askRow}
          {asked}
          onAsk={askClicked}
          head={L.head}
          thin={L.portrait}
          {wavesVersion}
          {spans}
          {wordEdges}
          edges={!reviewing}
          onView={setView}
          onSelect={(s) => (selection = s)}
          onError={(e) => (failure = e)}
        />
      </div>
    {/if}
    {#if reviewing}
      <Review bind:this={reviewer} {player} {edit} only={reviewOnly} at={reviewAt} onDone={() => (reviewing = false)} onError={(e) => (failure = e)} />
    {:else}
      <ActionBar
        {player}
        {edit}
        {selection}
        wordsShown={panelWidth > 0 && tab === "transcript"}
        at={toolsAt} onSelect={(s) => (selection = s)} onError={(e) => (failure = e)} />
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
        <button role="tab" aria-selected={tab === "moments"} class:on={tab === "moments"} onclick={() => (tab = "moments")}
          >Moments{moments?.moments.some((m) => m.state === "suggested") ? ` (${moments.moments.filter((m) => m.state === "suggested").length})` : ""}</button
        >
        <button role="tab" aria-selected={tab === "like"} class:on={tab === "like"} onclick={() => (tab = "like")}>Like this</button>
        <button role="tab" aria-selected={tab === "ask"} class:on={tab === "ask"} onclick={() => (tab = "ask")}
          >Ask{asked?.steps.some((s) => s.ghosts.length) ? " •" : ""}</button
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
          error={analysis?.phase === "failed" ? analysis.error : null}
          file={opened?.file ?? null}
          {through}
          {player}
          {edit}
          {selection}
          {flagBelow}
          {reviewing}
          onSelect={(s) => (selection = s)}
          onError={(e) => (failure = e)}
          {correct}
        />
      </div>
      {#if tab === "moments"}
        <div class="pane">
          <MomentsPanel
            view={moments}
            editable={!!edit?.editable}
            focus={momentFocus}
            onAnswer={momentAnswer}
            onJump={(m) => void jumpTo(m)}
            onRollCall={() => (rollCallOpen = true)}
          />
        </div>
      {:else if tab === "ask"}
        <div class="pane">
          <AskPanel
            view={asked}
            editable={!!edit?.editable}
            {spans}
            previewing={!!edit?.preview}
            {asking}
            focus={askFocus}
            onAsk={askRequest}
            onRerun={askRerun}
            onAnswer={askAnswer}
            onPreview={askPreview}
            onPlay={(t) => void askPlay(t)}
          />
        </div>
      {:else if tab === "like"}
        <div class="pane">
          <LikePanel
            view={like}
            editable={!!edit?.editable}
            {spans}
            playhead={() => player?.position ?? null}
            tracks={likeTracks}
            focus={likeFocus}
            searching={likeSearching}
            onSettings={likeSettings}
            onSearch={likeSearch}
            onAct={likeAct}
            onAnswer={likeAnswer}
            onPlay={(s) => void likePlay(s)}
          />
        </div>
      {:else if tab === "captions"}
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
            {keep}
            keepFile={opened.file}
            onKeepChanged={() => void loadKeep()}
            onReviewUncertain={reviewUncertain}
            onError={(e) => (failure = e)}
          />
        </div>
      {/if}
    </div>
  {/if}

  {#if settingsOpen}
    <SettingsDialog onClose={() => (settingsOpen = false)} onSaved={settingsSaved} />
  {/if}
  {#if rollCallOpen && opened}
    <RollCall file={opened.file} onClose={rollCallClosed} onError={(e) => (failure = e)} />
  {/if}
  {#if correcting}
    <ReanalyseDialog selection={reanalyseSpan} onRun={reanalyse} onClose={() => closeCorrect()} />
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
