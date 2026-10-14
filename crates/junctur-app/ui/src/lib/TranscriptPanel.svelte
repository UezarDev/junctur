<script lang="ts">
  // The transcript beside the video: kept words in order, cut runs folded, the word under the
  // playhead lit and followed. Rows have one fixed height and only those on screen (plus a few) are
  // in the DOM, so a 30 h transcript scrolls like a short one. An edit rebuilds the tokens
  // (transcript.ts, one pass over the words); a resize or a fold toggle only repacks them into rows.
  // Where the speaker changes a name row comes first; click the name to rename the speaker (saved
  // to edit.junctur at once). Double-click a word to fix its spelling (5h): saved to edit.junctur at
  // once, shown here, in the caption preview and the SRT, and added to the word list Whisper is told
  // (the header says so for a moment, 5t).
  // A selection of words shows its actions at the bottom (WordBar, 2p): Cut, Keep, Fix text (5x),
  // and the people with names as one-click buttons. Ctrl+click or Ctrl+drag adds another stretch to
  // it. Several words fixed as one text (5x: a sentence retyped) show as that text, selected and lit
  // whole; a double-click inside a selection of several words, on such a text, or on a word starting
  // in the same millisecond as its neighbours (5w) opens the word bar's field for all of them.
  // A fix lands on every other word heard the same way too (5aa); a line under the header says how
  // many, with Undo those. The word bar's Correct does that for every fix, then hears the speakers and
  // the words again (App asks first and starts the run).
  // Words Whisper was unsure of are underlined (5o) and stepped through from the header; "Looks
  // right", which confirms the one flagged word selected, is among the word actions at the bottom
  // (2p's place for them). A fixed or confirmed word, or one in a retyped text, is not flagged (the
  // core decides, `words.sure`).
  import { onMount } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import * as api from "./api";
  import * as sel from "./selection";
  import { toSeconds } from "./time";
  import {
    flagged,
    pack,
    stamp,
    stepFrom,
    together,
    tokens,
    widen,
    wordAt,
    type Stretch,
    type Token,
    type Words,
  } from "./transcript";
  import WordBar from "./WordBar.svelte";

  let {
    words,
    loading,
    analysing = false,
    listed = false,
    waiting = false,
    failed = false,
    through = null,
    player,
    edit,
    selection,
    flagBelow = 0,
    reviewing = false,
    onSelect,
    onError,
    correct,
  }: {
    words: Words | null;
    loading: boolean;
    /** The recording is being analysed: the transcript comes when it is done. */
    analysing?: boolean;
    /** Its edit list is made: the transcript comes a 5-minute piece at a time (2j). */
    listed?: boolean;
    /** Its transcription waits for another recording's (2j). */
    waiting?: boolean;
    /** Its analysis failed (a transcription that stopped part way keeps its pieces). */
    failed?: boolean;
    /** While it transcribes: the words shown are every one before this many seconds (2j). */
    through?: number | null;
    player: api.PlayerState | null;
    edit: api.EditState | null;
    selection: sel.Selection | null;
    /** Kept words Whisper was less sure of than this many percent are flagged (5o); 0 for none. */
    flagBelow?: number;
    /** Review mode (2o) has the editing: the word bar is not shown meanwhile. */
    reviewing?: boolean;
    onSelect: (s: sel.Selection | null) => void;
    onError: (message: string) => void;
    /** The Correct button (5aa): asks, starts the run; its answer, or null when not started. */
    correct: () => Promise<api.Corrected | null>;
  } = $props();

  const ROW = 24;
  const GUTTER = 58;
  const PAD = 12;
  // Rows rendered beyond the visible ones, so a fast scroll does not show blanks.
  const OVERSCAN = 12;
  // After the user scrolls, the list stops following the playhead for this long.
  const FOLLOW_PAUSE_MS = 4000;
  // A window drag resizes every frame; rows are repacked once it settles.
  const RESIZE_SETTLE_MS = 120;
  const FONT = '14px "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif';
  const SMALL = '12px "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif';
  const SPACE = 4;

  let scroller: HTMLDivElement;
  let width = $state(0);
  let height = $state(0);
  let top = $state(0);
  const open = new SvelteSet<number>();

  // A new transcript (another recording) starts with every fold closed. One that only grew at its
  // end (2j: a piece more while it transcribes) keeps them: folds are keyed by their first word,
  // whose index has not moved.
  let shown: Words | null = null;
  $effect(() => {
    const w = words;
    const grew = !!w && !!shown && w.text.length >= shown.text.length && w.start[0] === shown.start[0];
    if (!grew) open.clear();
    shown = w;
  });

  // Text widths measured once per distinct string (speech repeats words a lot).
  const ctx = document.createElement("canvas").getContext("2d")!;
  const cache = new Map<string, number>();
  function measure(text: string, font: string): number {
    const key = font === FONT ? text : `\u0001${text}`;
    let w = cache.get(key);
    if (w === undefined) {
      ctx.font = font;
      w = ctx.measureText(text).width;
      cache.set(key, w);
    }
    return w;
  }

  function seconds(frames: Stretch): number {
    return edit?.fps ? toSeconds(frames.end - frames.start, edit.fps) : 0;
  }

  function foldLabel(t: Extract<Token, { kind: "cut" }>, isOpen: boolean): string {
    const n = t.last - t.first + 1;
    return `${isOpen ? "▾" : "▸"} cut ${seconds(t.frames).toFixed(1)} s, ${n} word${n === 1 ? "" : "s"}`;
  }

  function markWidth(t: Token): number {
    switch (t.kind) {
      case "word":
      case "cutword":
      case "spanword":
        return 0; // measured per transcript, in `wordWidth` and `spanWidth`
      case "cut": {
        // The wider of the two labels (open or closed), the padding, and the keep button.
        const label = Math.max(measure(foldLabel(t, true), SMALL), measure(foldLabel(t, false), SMALL));
        return label + measure("keep", SMALL) + 34 + SPACE;
      }
      case "gap":
        return 10 + SPACE;
    }
  }

  // Only a slow step is logged (test mode reads the log; a 30 h transcript is the case to watch).
  const SLOW_MS = 50;
  function timed<T>(what: string, f: () => T): T {
    const t0 = performance.now();
    const out = f();
    const ms = performance.now() - t0;
    if (ms > SLOW_MS) void api.uiLog(`slow transcript ${what}: ${ms.toFixed(0)} ms`).catch(() => {});
    return out;
  }

  // Per transcript: each word's width.
  const wordWidth = $derived(
    words ? timed("widths", () => Float64Array.from(words!.text, (t) => measure(t, FONT) + SPACE)) : null,
  );
  // Per transcript: each span text's words' widths (5x).
  const spanWidth = $derived(
    words ? words.spanWords.map((ws) => ws.map((t) => measure(t, FONT) + SPACE)) : [],
  );
  // Per edit: which words are kept, the folds.
  const toks = $derived(
    words && wordWidth ? timed("tokens", () => tokens(words!, wordWidth!, edit, markWidth, spanWidth)) : null,
  );
  // Per resize or fold toggle: rows only.
  const laid = $derived(
    toks && words && width > 0
      ? timed("rows", () =>
          pack(toks, words!.text.length, open, Math.max(80, width - GUTTER - 2 * PAD), words!.speaker),
        )
      : null,
  );

  // Speakers: names and caption colours from the edit list (set in the Captions panel).
  const names = $derived(new Map((edit?.speakers ?? []).map((s) => [s.id, s.name])));
  const colours = $derived(new Map((edit?.speakers ?? []).map((s) => [s.id, s.color])));
  const colour = (id: number) => colours.get(id) ?? "var(--muted)";
  const nameOf = (id: number) => names.get(id) ?? `Speaker ${id}`;

  // Renaming: the one name row that was clicked (by row index: the same speaker has a name row at
  // every turn) turns into an input; Enter or leaving it saves, Escape drops it. While it is open
  // the list stops following the playhead, so the row is not scrolled away (and unmounted) under
  // the user's typing.
  let renaming = $state<{ id: number; row: number } | null>(null);
  let draft = $state("");
  function startRename(id: number, row: number) {
    renaming = { id, row };
    draft = nameOf(id);
  }
  function focusInput(el: HTMLInputElement) {
    el.focus();
    el.select();
  }
  function finishRename(save: boolean) {
    const id = renaming?.id;
    renaming = null;
    if (!save || id == null) return;
    const name = draft.trim();
    if (name && name !== nameOf(id)) call(() => api.renameSpeaker(id, name));
  }

  // Fixing a word (5h): the one double-clicked turns into a field with its text; Enter or leaving it
  // saves, Escape drops it. Nothing typed, or the heard text again, takes a fix away (the core
  // decides). The new text comes back through the edit state's fixes key (App.svelte).
  let fixing = $state<number | null>(null);
  let fixDraft = $state("");
  // `fixing` as a plain variable, for the field's own teardown and blur (5t repair): Svelte gives an
  // effect's teardown the state from before the change that tore it down, so there `fixing` still
  // named the field just closed. Its teardown then finished it a second time: Escape's draft was
  // saved after all, Enter saved twice, and a field replaced by another word's closed that one too.
  let fixOpen: number | null = null;
  // The word bar's field for several words (5x), opened from here by a double-click. Closed whenever
  // the selection stops being words (5x repair): the bar is gone then, and a field left marked open
  // would come back unasked with the next word clicked.
  let retyping = $state(false);
  $effect(() => {
    if (selection?.kind !== "words") retyping = false;
  });
  // The selection before a press, for a double-click inside a selection of several words: the
  // double-click's first press had already replaced it with the one word pressed. `pending` is the
  // selection as each press found it; the click after a first press (`detail` 1) keeps it, the
  // second click of a double-click (`detail` 2) does not, so the double-click reads the selection
  // from before its first press. The click counts the clicks with the system's own double-click
  // time (5x repair: a 600 ms guess of ours was shorter than Windows allows); a pointerdown's
  // `detail` is always 0 in WebView2, so the press itself cannot.
  let beforePress: sel.Selection | null = null;
  let pending: sel.Selection | null = null;
  function clicked(e: MouseEvent) {
    if (e.detail <= 1) beforePress = pending;
  }
  function startFix(e: MouseEvent) {
    // Not `e.target` (5t): `down` captures the pointer on the scroller, and Chromium (WebView2) then
    // sends the clicks and the double-click to the scroller, so the target never was a word and the
    // field never opened. The element under the pointer, as `move` finds it. Only a kept word: a cut
    // word in an open fold has no field, and `fixing` set without one would hold the follow for good.
    const el = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>("[data-i]");
    if (!el || el.classList.contains("cutword") || !words) return;
    const i = Number(el.dataset.i);
    if (i === fixing) return;
    // Several words, one field in the word bar (5x): a span fixed as one text, a selection of
    // several words holding this one, or the words starting with it in one millisecond on its track.
    const s = words.span[i];
    const prior = sel.runs(beforePress);
    const run = s
      ? { first: words.spanFirst[s - 1], last: words.spanLast[s - 1] }
      : prior.length === 1 && prior[0].last > prior[0].first && sel.has(prior, i)
        ? prior[0]
        : together(words, i);
    if (run) {
      if (fixing != null) finishFix(true);
      // Whole spans: a group or selection reaching into one takes all of it, so the span is
      // retyped, not cut short by the save (which takes away every fix it touches).
      const whole = widen(words, run);
      onSelect(sel.words(whole.first, whole.last));
      retyping = true;
      return;
    }
    // Another word's field still open (the press on this word kept it from blurring): what was
    // typed there is saved first, as leaving it does (5t repair: it was lost, or saved as this
    // word's text, when this word's draft replaced it before its field went away).
    if (fixing != null) finishFix(true);
    fixing = fixOpen = i;
    fixDraft = words.text[i];
  }
  // The field's row can scroll out of the rendered window (the list is virtual) and take the field
  // with it, without a blur: what was typed is saved then, as leaving the field does, so `fixing`
  // never outlives its field (it holds the playhead follow). Only while the fix is still this
  // field's own: one replaced by another word's field was saved by `startFix` already.
  function fixField(el: HTMLInputElement, i: number) {
    focusInput(el);
    return {
      destroy() {
        if (fixOpen === i) finishFix(true);
      },
    };
  }
  function finishFix(save: boolean) {
    const i = fixing;
    fixing = fixOpen = null;
    if (!save || i == null || !words || fixDraft.trim() === words.text[i]) return;
    const text = fixDraft;
    call(async () => tellFixed(await api.fixWord(i, text)));
  }

  // A fix whose spelling joined the word list (5h's `settings::learn`) says so for a moment in the
  // header (5t): before, nothing showed the list had changed until Settings was opened. A retyped
  // sentence (5x) names every word it added.
  const LEARNED_MS = 4000;
  let learned = $state<string | null>(null);
  let learnedTimer = 0;
  // Cleared when the panel goes: `fixWord` can answer after that (5t repair), and a timer set then
  // would outlive the panel.
  let mounted = false;
  function tellLearned(all: string[]) {
    if (!mounted || !all.length) return;
    learned = all.map((e) => `"${e}"`).join(", ");
    clearTimeout(learnedTimer);
    learnedTimer = window.setTimeout(() => (learned = null), LEARNED_MS);
  }

  // What a fix also did elsewhere (5aa), under the header for a while: "Also fixed 2 other places"
  // with Undo those (the engine takes back only those still as it made them), or what Correct
  // started. A newer fix's line replaces it.
  // Correct's line stays until dismissed: its run goes on for a while, and its Undo with it.
  const NOTE_MS = 20000;
  let note = $state<{ text: string; others: api.Fix[]; heard: string[] } | null>(null);
  let noteTimer = 0;
  function tell(text: string, others: api.Fix[] = [], heard: string[] = [], stay = false) {
    if (!mounted) return;
    note = { text, others, heard };
    clearTimeout(noteTimer);
    if (!stay) noteTimer = window.setTimeout(() => (note = null), NOTE_MS);
  }
  function places(n: number): string {
    return `${n.toLocaleString()} other place${n === 1 ? "" : "s"}`;
  }
  function tellFixed(r: api.Fixed) {
    tellLearned(r.learned);
    if (r.others.length) tell(`Also fixed ${places(r.others.length)} heard the same way.`, r.others, r.heard);
  }
  // Kept as heard rather than taken away (5aa): a later Correct, or the spread after its
  // transcription, would otherwise give those words the fix again.
  async function undoOthers() {
    const n = note;
    note = null;
    if (!n?.others.length) return;
    await call(async () => {
      if ((await api.keepHeard(n.others, n.heard)) === 0) {
        tell("Nothing to undo: those fixes have moved onto the new transcript, or were changed since.");
      }
    });
  }

  // Fixes a re-transcription could not put on any word (5aa repair): listed once it ends, and on
  // opening, until dismissed, so a fix is never lost without the user seeing it.
  let setAside = $state<api.SetAside[]>([]);
  $effect(() => {
    if (analysing) return;
    void edit?.fixesKey;
    api.fixesSetAside().then(
      (s) => (setAside = s),
      () => (setAside = []),
    );
  });
  async function forgetSetAside() {
    // Only what was shown (5ad): a run may have set more aside since, which stay to be seen.
    const seen = setAside;
    setAside = [];
    await call(() => api.forgetFixesSetAside(seen));
  }
  // A set-aside fix, or a word time set by moving a cut's edge (5ad), as the line lists it.
  function asideText(s: api.SetAside): string {
    const silent = s.silent ? ", its track has no words this time" : "";
    if (s.retimed) {
      return `the time of "${s.text}" (${stamp(s.at * 1000)}, set to ${stamp(s.retimed[0] * 1000)} to ${stamp(s.retimed[1] * 1000)}${silent})`;
    }
    return `"${s.text}" (was "${s.heard}", ${stamp(s.at * 1000)}${silent})`;
  }
  // The set-aside line (5ad repair): the fixes, and what Correct could not do (`note`), which the
  // run's progress line no longer shows by its end.
  const asideLine = $derived.by(() => {
    const fixes = setAside.filter((s) => !s.note);
    const notes = setAside.flatMap((s) => (s.note ? [s.note] : []));
    const listed = fixes.length
      ? `Set aside, not on any word now: ${fixes.map(asideText).join(", ")}. Fix those words again where they are now.`
      : "";
    return [listed, ...notes].filter((t) => t).join(" ");
  });
  async function correctAll() {
    const r = await correct();
    if (!r) return;
    const unclear = r.unclear.length
      ? ` Fixed two ways, left as heard: ${r.unclear.map((u) => `"${u}"`).join(", ")}.`
      : "";
    const n = r.others.length;
    tell(
      `Correcting: ${n ? `fixed ${places(n)} heard the same way; ` : ""}hearing the speakers and the words again (Properties shows how far).${unclear}`,
      r.others,
      r.heard,
      true,
    );
  }

  const first = $derived(Math.max(0, Math.floor(top / ROW) - OVERSCAN));
  const visible = $derived.by(() => {
    if (!laid) return [];
    const last = Math.min(laid.rows.length, Math.ceil((top + height) / ROW) + OVERSCAN);
    const out = [];
    for (let i = first; i < last; i++) out.push({ index: i, row: laid.rows[i] });
    return out;
  });

  const current = $derived(words && player?.position != null ? wordAt(words, player.position * 1000) : -1);
  // The span the playhead is in (5x), lit whole.
  const currentSpan = $derived(words && current >= 0 ? (words.span[current] ?? 0) : 0);
  // The selected words, sorted and joined, for drawing; the latest run, for Shift+click.
  const joined = $derived(sel.runs(selection));
  const latest = $derived(sel.latest(selection));

  // Flagged words (5o): per edit or threshold, the kept ones to check, in order.
  const toCheck = $derived(toks && words ? flagged(toks, words.sure, flagBelow) : []);
  const doubt = (i: number) => flagBelow > 0 && words != null && words.sure[i] < flagBelow;
  // The one word selected (a single stretch of one word), when it is flagged: what "Looks right"
  // confirms.
  const checking = $derived(
    joined.length === 1 && joined[0].first === joined[0].last && doubt(joined[0].first) ? joined[0].first : -1,
  );

  // Next or previous flagged word from the selection, else from the word under the playhead: it is
  // selected, played from and scrolled to (even while the list does not follow the playhead). Each
  // button is greyed when there is none that way (no wrapping: the end of the list stays an end).
  // With several stretches selected (2p), from the first one's start and the last one's end.
  const before = $derived(stepFrom(toCheck, joined.length ? joined[0].first : current, true));
  const after = $derived(stepFrom(toCheck, joined.length ? joined[joined.length - 1].last : current, false));
  function goTo(i: number) {
    if (i < 0 || !words) return;
    anchor = i;
    onSelect(sel.words(i, i));
    call(() => api.seek(words!.start[i] / 1000, true));
    if (laid && scroller) {
      ours = Math.max(0, Math.round(laid.rowOf[i] * ROW - height / 3));
      scroller.scrollTop = ours;
      pausedUntil = performance.now() + FOLLOW_PAUSE_MS;
    }
  }
  // Confirms the selected flagged word (it keeps what Whisper heard) and goes on to the next.
  function looksRight() {
    const i = checking;
    if (i < 0) return;
    const next = stepFrom(toCheck, i, false);
    call(async () => {
      await api.confirmWord(i);
      goTo(next);
    });
  }

  // Following the playhead: scroll so the current word's row stays in view, unless the user has
  // scrolled lately. Our own scrolls are told apart by the position they set.
  let pausedUntil = 0;
  let ours = -1;
  $effect(() => {
    if (!laid || current < 0 || !scroller) return;
    const row = laid.rowOf[current];
    if (renaming || fixing != null || performance.now() < pausedUntil) return;
    const y = row * ROW;
    if (y < top + ROW || y > top + height - 3 * ROW) {
      ours = Math.max(0, Math.round(y - height / 3));
      scroller.scrollTop = ours;
    }
  });

  function onScroll() {
    top = scroller.scrollTop;
    if (ours < 0 || Math.abs(top - ours) > 1) pausedUntil = performance.now() + FOLLOW_PAUSE_MS;
    ours = -1;
  }

  async function call(f: () => Promise<unknown>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }

  // Selecting: press on a word, drag across others, release. A press and release on one word
  // without moving seeks there (and selects it); shift extends the latest stretch from its anchor.
  // Ctrl (2p; Cmd on macOS, where Ctrl+click is the context menu, and never the Windows key)
  // adds a stretch beside the ones selected: a press on a word adds it (a drag from it, the words
  // dragged over), and a press and release on a word already selected takes it out, while a drag
  // that starts on a selected word adds the words dragged over like any other (it only takes out
  // the one word when the press does not move); a Ctrl+click does not seek, so picking out someone's lines one after another leaves the playhead
  // where it was. `pressed` is the one drag in progress; `anchor` outlives it, for shift-clicks.
  // `base` is what the drag's own run goes beside: nothing for a plain press, every stretch for
  // Ctrl, all but the latest for Shift (that one is the run being extended).
  let anchor = -1;
  let pressed = false;
  let moved = false;
  let base: sel.Selection | null = null;
  let adding = false;
  let unpick = false;
  const MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
  function wordUnder(target: EventTarget | Element | null): number {
    const el = (target as HTMLElement | null)?.closest?.<HTMLElement>("[data-i]");
    return el ? Number(el.dataset.i) : -1;
  }

  // Selecting a run that reaches into a span takes the whole span (5x).
  function pick(s: sel.Selection | null) {
    if (s?.kind === "words" && words) {
      const w = words;
      onSelect({ kind: "words", runs: s.runs.map((r) => widen(w, r)) });
    } else onSelect(s);
  }

  function down(e: PointerEvent) {
    pressed = false;
    pending = selection;
    const i = wordUnder(e.target);
    if (i < 0 || e.button !== 0) return;
    e.preventDefault();
    // The press seeks to the word on release (`up`), and the follow would then scroll a word in the
    // first row or the last three away from under the pointer before a double-click's second press
    // (5t). The word pressed is on screen already.
    pausedUntil = performance.now() + FOLLOW_PAUSE_MS;
    adding = MAC ? e.metaKey : e.ctrlKey;
    unpick = false;
    moved = false;
    if (adding) {
      base = selection?.kind === "words" ? selection : null;
      anchor = i;
      if (sel.has(joined, i)) unpick = true;
      else pick(sel.adding(base, sel.run(i, i)));
    } else if (e.shiftKey && latest && selection?.kind === "words") {
      // The anchor only counts while it is an end of the latest stretch (the timeline may have
      // replaced it).
      const from = anchor === latest.first || anchor === latest.last ? anchor : latest.first;
      anchor = from;
      base = { kind: "words", runs: selection.runs.slice(0, -1) };
      pick(sel.adding(base, sel.run(from, i)));
      moved = true;
    } else {
      anchor = i;
      base = null;
      pick(sel.words(i, i));
    }
    pressed = true;
    scroller.setPointerCapture(e.pointerId);
  }

  function move(e: PointerEvent) {
    if (!pressed || !(e.buttons & 1)) return;
    // Captured: the target is the scroller, so find the word under the pointer instead.
    const i = wordUnder(document.elementFromPoint(e.clientX, e.clientY));
    if (i >= 0 && (i !== anchor || moved)) {
      moved = true;
      pick(sel.adding(base, sel.run(anchor, i)));
    }
  }

  function up() {
    if (!pressed) return;
    pressed = false;
    if (!moved && unpick) {
      // A span is taken out whole (5x).
      const r = words ? widen(words, { first: anchor, last: anchor }) : { first: anchor, last: anchor };
      let left = selection;
      for (let j = r.first; j <= r.last; j++) left = sel.without(left, j);
      onSelect(left);
    }
    else if (!moved && !adding && words && anchor >= 0) {
      const i = anchor;
      call(() => api.seek(words!.start[i] / 1000, true));
    }
    moved = false;
    unpick = false;
  }

  function toggle(t: Extract<Token, { kind: "cut" }>) {
    if (open.has(t.first)) open.delete(t.first);
    else open.add(t.first);
  }

  function restore(frames: Stretch) {
    call(() => api.editFrames(frames.start, frames.end, true));
  }

  onMount(() => {
    mounted = true;
    let timer = 0;
    const ro = new ResizeObserver(() => {
      height = scroller.clientHeight;
      clearTimeout(timer);
      // The first size at once, later ones once the resize settles.
      if (width === 0) width = scroller.clientWidth;
      else timer = window.setTimeout(() => (width = scroller.clientWidth), RESIZE_SETTLE_MS);
    });
    ro.observe(scroller);
    return () => {
      clearTimeout(timer);
      mounted = false;
      clearTimeout(learnedTimer);
      clearTimeout(noteTimer);
      ro.disconnect();
    };
  });
</script>

<section class="panel">
  <header>
    <span>Transcript</span>
    {#if toCheck.length}
      <span class="check" title="Words Whisper was less sure of than {flagBelow} % (Settings)">
        <button onclick={() => goTo(before)} disabled={before < 0} aria-label="Previous word to check">‹</button>
        <span>{toCheck.length.toLocaleString()} to check</span>
        <button onclick={() => goTo(after)} disabled={after < 0} aria-label="Next word to check">›</button>
      </span>
    {/if}
    {#if learned}
      <span class="muted learned" role="status">{learned} added to the word list</span>
    {:else if words}
      <span class="muted">{words.text.length.toLocaleString()} words</span>
    {/if}
  </header>
  {#if setAside.length}
    <div class="spread" role="status">
      <span class="grow"
        >{asideLine}</span
      >
      <button class="x" onclick={() => void forgetSetAside()} aria-label="Dismiss" title="Seen: stop listing them">×</button>
    </div>
  {/if}
  {#if note}
    <div class="spread" role="status">
      <span class="grow">{note.text}</span>
      {#if note.others.length}
        <button
          disabled={analysing && !listed}
          onclick={() => void undoOthers()}
          title={analysing && !listed
            ? "Once the speakers are heard again (the edit list is busy until then)"
            : "Leave the other places as they were heard; the one you typed stays fixed"}>Undo those</button
        >
      {/if}
      <button class="x" onclick={() => (note = null)} aria-label="Dismiss">×</button>
    </div>
  {/if}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div
    class="scroll"
    bind:this={scroller}
    onscroll={onScroll}
    onwheel={() => (pausedUntil = performance.now() + FOLLOW_PAUSE_MS)}
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onclick={clicked}
    ondblclick={startFix}
  >
    {#if loading}
      <p class="note">Reading the transcript…</p>
    {:else if !words || (through != null && words.text.length === 0)}
      <p class="note">
        {waiting
          ? "Waiting to transcribe: another recording's transcription comes first. Cut and keep on the timeline meanwhile; the words show here five minutes at a time once it starts."
          : analysing && listed
            ? "Transcribing: the words show here five minutes at a time. Cut and keep on the timeline meanwhile."
            : analysing
              ? "Analysing this recording: the transcript shows here when it is done."
              : "No transcript for this recording yet. Re-analyse it (project panel) to edit by text."}
      </p>
    {:else if laid}
      <div class="spacer" style:height="{laid.rows.length * ROW}px">
        {#each visible as { index, row } (index)}
          <div class="row" style:top="{index * ROW}px" style:padding-left="{GUTTER}px">
            {#if row.speaker != null}
              {@const id = row.speaker}
              {#if renaming?.row === index}
                <input
                  class="rename"
                  maxlength="60"
                  bind:value={draft}
                  use:focusInput
                  onpointerdown={(e) => e.stopPropagation()}
                  onkeydown={(e) => {
                    e.stopPropagation();
                    if (e.key === "Enter") finishRename(true);
                    else if (e.key === "Escape") finishRename(false);
                  }}
                  onblur={() => finishRename(true)}
                  aria-label="Speaker name"
                />
              {:else}
                <button
                  class="speaker"
                  style:color={colour(id)}
                  onpointerdown={(e) => e.stopPropagation()}
                  onclick={() => startRename(id, index)}
                  title="Rename this speaker">{nameOf(id)}</button
                >
              {/if}
            {/if}
            {#if row.para != null}<span class="time">{stamp(row.para)}</span>{/if}
            {#each row.tokens as k (k)}
              {@const t = toks!.list[k]}
              {#if t.kind === "word" && t.i === fixing}
                <input
                  class="fix"
                  maxlength="100"
                  style:width="{Math.max(60, (wordWidth?.[t.i] ?? 0) + 24)}px"
                  bind:value={fixDraft}
                  use:fixField={t.i}
                  onpointerdown={(e) => e.stopPropagation()}
                  ondblclick={(e) => e.stopPropagation()}
                  onkeydown={(e) => {
                    e.stopPropagation();
                    if (e.key === "Enter") finishFix(true);
                    else if (e.key === "Escape") finishFix(false);
                  }}
                  onblur={() => fixOpen === t.i && finishFix(true)}
                  aria-label="Fix this word's spelling"
                />
              {:else if t.kind === "word"}
                <span
                  class="w"
                  class:cur={t.i === current}
                  class:sel={sel.has(joined, t.i)}
                  class:doubt={doubt(t.i)}
                  style:box-shadow={words.also[t.i] ? `inset 0 -2px 0 ${colour(words.also[t.i])}` : null}
                  title={words.also[t.i]
                    ? `${nameOf(words.speaker[t.i])} and ${nameOf(words.also[t.i])} at once`
                    : doubt(t.i)
                      ? `Whisper was ${words.sure[t.i]} % sure of this word: double-click to fix it, or select it and "Looks right" below`
                      : "Double-click to fix the spelling"}
                  data-i={t.i}>{words.text[t.i]}</span
                >
              {:else if t.kind === "spanword"}
                <span
                  class="w fixed"
                  class:cur={currentSpan === t.s}
                  class:sel={sel.has(joined, t.i)}
                  title="Fixed as one text: double-click to change it"
                  data-i={t.i}>{words.spanWords[t.s - 1][t.k]}</span
                >
              {:else if t.kind === "cutword"}
                <span
                  class="w cutword"
                  class:cur={t.i === current}
                  class:sel={sel.has(joined, t.i)}
                  data-i={t.i}>{words.text[t.i]}</span
                >
              {:else if t.kind === "cut"}
                <span class="fold">
                  <button class="label" onclick={() => toggle(t)} title="Show or hide the cut words"
                    >{foldLabel(t, open.has(t.first))}</button
                  ><button class="keep" onclick={() => restore(t.frames)} title="Keep this cut stretch again"
                    >keep</button
                  >
                </span>
              {:else}
                <button
                  class="gap"
                  onclick={() => restore(t.frames)}
                  title="Cut {seconds(t.frames).toFixed(1)} s with no words; click to keep it"
                  aria-label="Keep this cut"
                ></button>
              {/if}
            {/each}
          </div>
        {/each}
      </div>
      {#if through != null}
        <p class="note">
          {failed
            ? `Transcribed to ${stamp(through * 1000)}; the transcription stopped there (Properties says why).`
            : `Transcribed to ${stamp(through * 1000)}; the rest follows as it is done.`}
        </p>
      {/if}
    {/if}
  </div>
  {#if selection?.kind === "words" && words && edit?.editable && !reviewing}
    <WordBar
      {edit}
      {words}
      {selection}
      {onSelect}
      {onError}
      bind:retyping
      onFixed={tellFixed}
      onCorrect={correctAll}
      onLooksRight={checking >= 0 ? looksRight : null}
    />
  {/if}
</section>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--panel-solid);
    border-left: 1px solid var(--line);
  }
  header {
    display: flex;
    justify-content: space-between;
    padding: 10px 12px 8px;
    font-weight: 600;
    border-bottom: 1px solid var(--line);
  }
  .muted {
    color: var(--muted);
    font-weight: 400;
  }
  .learned {
    min-width: 0;
    margin-left: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spread {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 12px;
    font-size: 12px;
    color: var(--muted);
    border-bottom: 1px solid var(--line);
  }
  .spread .grow {
    flex: 1;
    min-width: 0;
  }
  .spread button {
    padding: 2px 8px;
    font-size: 12px;
  }
  .scroll {
    position: relative;
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
  }
  .note {
    margin: 16px 12px;
    color: var(--muted);
  }
  .spacer {
    position: relative;
  }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    height: 24px;
    line-height: 24px;
    padding-right: 12px;
    white-space: nowrap;
    overflow: hidden;
    font: 14px "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif;
  }
  .time {
    position: absolute;
    left: 12px;
    color: var(--muted);
    font: 11px "Cascadia Mono", Consolas, monospace;
  }
  .w {
    margin-right: 4px;
    border-radius: 3px;
    cursor: pointer;
  }
  .w:hover {
    background: rgba(255, 255, 255, 0.08);
  }
  .w.sel {
    background: rgba(242, 179, 61, 0.28);
  }
  .w.fixed {
    text-decoration: underline dotted rgba(255, 255, 255, 0.35);
    text-underline-offset: 3px;
  }
  .w.cur {
    color: #101114;
    background: var(--accent);
  }
  .w.doubt {
    text-decoration: underline wavy rgba(242, 179, 61, 0.9);
    text-decoration-thickness: 1px;
    text-underline-offset: 4px;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-weight: 400;
    font-size: 12px;
    color: var(--muted);
  }
  .check button {
    padding: 0 6px;
    font-size: 12px;
    line-height: 18px;
  }
  .cutword {
    color: var(--muted);
    text-decoration: line-through;
  }
  .fold {
    display: inline-flex;
    margin-right: 4px;
    vertical-align: middle;
  }
  .fold button {
    font-size: 12px;
    line-height: 16px;
    padding: 1px 6px;
    color: var(--muted);
    background: rgba(239, 111, 94, 0.12);
    border: 1px solid rgba(239, 111, 94, 0.35);
  }
  .fold .label {
    border-radius: 4px 0 0 4px;
  }
  .fold .keep {
    border-radius: 0 4px 4px 0;
    border-left: none;
  }
  .gap {
    display: inline-block;
    width: 4px;
    height: 14px;
    margin: 0 7px 0 3px;
    padding: 0;
    vertical-align: middle;
    border: none;
    border-radius: 2px;
    background: rgba(239, 111, 94, 0.45);
  }
  .gap:hover {
    background: var(--bad);
  }
  .speaker {
    padding: 0 4px;
    font-size: 12px;
    font-weight: 600;
    line-height: 20px;
    background: none;
    border: none;
    border-radius: 3px;
    cursor: text;
  }
  .speaker:hover {
    background: rgba(255, 255, 255, 0.08);
  }
  .fix {
    height: 20px;
    margin-right: 4px;
    font: 14px "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif;
  }
  .rename {
    width: 220px;
    height: 20px;
    font-size: 12px;
    font-weight: 600;
  }
</style>
