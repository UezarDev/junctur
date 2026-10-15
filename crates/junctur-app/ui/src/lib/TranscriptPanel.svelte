<script lang="ts">
  // The transcript beside the video: kept words in order, cut runs folded, the playhead's word lit
  // and followed. Rows have one fixed height and only those on screen are in the DOM, so a long
  // transcript scrolls like a short one; a resize or fold toggle only repacks the rows.
  import { onMount, tick } from "svelte";
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
    wordRef,
    type Stretch,
    type Token,
    type Words,
  } from "./transcript";
  import FixesList from "./FixesList.svelte";
  import WordBar from "./WordBar.svelte";

  let {
    words,
    loading,
    analysing = false,
    listed = false,
    waiting = false,
    failed = false,
    error = null,
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
    /** Its edit list is made: the transcript comes a 5-minute piece at a time. */
    listed?: boolean;
    /** Its transcription waits for another recording's. */
    waiting?: boolean;
    /** Its analysis failed (a transcription that stopped part way keeps its pieces). */
    failed?: boolean;
    /** Why it failed, when it did (the run's error, as Properties shows it). */
    error?: string | null;
    /** While it transcribes: the words shown are every one before this many seconds. */
    through?: number | null;
    player: api.PlayerState | null;
    edit: api.EditState | null;
    selection: sel.Selection | null;
    /** Kept words Whisper was less sure of than this many percent are flagged; 0 for none. */
    flagBelow?: number;
    /** Review mode has the editing: the word bar is not shown meanwhile. */
    reviewing?: boolean;
    onSelect: (s: sel.Selection | null) => void;
    onError: (message: string) => void;
    /** The Correct button: opens the pop-up; what its run started and what it is doing, or null
     * when none started. */
    correct: () => Promise<{ r: api.Corrected; doing: string } | null>;
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
  // end keeps them: folds are keyed by their first word, whose index has not moved.
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

  // Only a slow step is logged (test mode reads the log).
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
  // Per transcript: each span text's words' widths.
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

  // Fixing a word: the one double-clicked turns into a field with its text; Enter or leaving it
  // saves, Escape drops it. Nothing typed, or the heard text again, takes a fix away (the core
  // decides). The new text comes back through the edit state's fixes key (App.svelte).
  let fixing = $state<number | null>(null);
  let fixDraft = $state("");
  // What the open field showed when it opened, and its word as the engine names it (5ae). Leaving
  // saves only what was typed, never measured against the word as it is by then: a field left
  // alone while its word changed under it (a re-transcription's words) wrote its old text back onto
  // that index on a click away. The engine refuses the save when the index names another word now.
  let fixFrom = "";
  let fixRef: api.WordRef = { at: 0, track: 0, nth: 0 };
  // `fixing` as a plain variable, for the field's own teardown and blur: Svelte gives an effect's
  // teardown the state from before the change, so a `$state` would still name the field just closed.
  let fixOpen: number | null = null;
  // The word bar's field for several words, opened from here by a double-click. Closed whenever the
  // selection stops being words, or it would come back unasked with the next word clicked.
  let retyping = $state(false);
  $effect(() => {
    if (selection?.kind !== "words") retyping = false;
  });
  // The selection before a press, so a double-click inside a selection of several words reads it
  // from before its first press. The click counts with the system's double-click time, since a
  // pointerdown's `detail` is always 0 in WebView2.
  let beforePress: sel.Selection | null = null;
  let pending: sel.Selection | null = null;
  function clicked(e: MouseEvent) {
    if (e.detail <= 1) beforePress = pending;
  }
  // The kept word at a point in this list, or -1. Not `e.target`: `down` captures the pointer on the
  // scroller, so Chromium sends the clicks there. Only a kept word, as a cut word has no field.
  function keptWordAt(x: number, y: number): number {
    const el = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-i]");
    if (!el || el.classList.contains("cutword") || !scroller?.contains(el)) return -1;
    return Number(el.dataset.i);
  }
  function startFix(e: MouseEvent) {
    // With Ctrl (Cmd on macOS) a double-click is two Ctrl+clicks that pick the word and unpick it,
    // so it opens no field.
    if (MAC ? e.metaKey : e.ctrlKey) return;
    const i = keptWordAt(e.clientX, e.clientY);
    if (i >= 0) fixAt(i, beforePress);
  }
  // Opens the field for word `i`: what a double-click on it opens, and F2 over it. `before` is the
  // selection the user had made: from before the double-click's first press, or as it is for F2.
  function fixAt(i: number, before: sel.Selection | null) {
    if (!words || i === fixing) return;
    // Several words, one field in the word bar: a span fixed as one text, a selection of several
    // words holding this one, or the words starting with it in one millisecond on its track.
    const s = words.span[i];
    const prior = sel.runs(before);
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
    // typed there is saved first, as leaving it does.
    if (fixing != null) finishFix(true);
    fixing = fixOpen = i;
    fixDraft = fixFrom = words.text[i];
    fixRef = wordRef(words, i);
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
    if (!save || i == null || fixDraft.trim() === fixFrom.trim()) return;
    const text = fixDraft;
    const word = fixRef;
    call(async () => tellFixed(await api.fixWord(i, word, text)));
  }

  // A fix whose spelling joined the word list says so for a moment in the header; a retyped
  // sentence names every word it added. The same place says when Looks right has none left.
  const LEARNED_MS = 4000;
  let learned = $state<string | null>(null);
  let learnedTimer = 0;
  // Cleared when the panel goes: `fixWord` can answer after that, and a timer set then would
  // outlive the panel.
  let mounted = false;
  // In the header for a moment, in place of the word count (`tell` is the line under it).
  function flash(text: string) {
    if (!mounted) return;
    learned = text;
    clearTimeout(learnedTimer);
    learnedTimer = window.setTimeout(() => (learned = null), LEARNED_MS);
  }
  function tellLearned(all: string[]) {
    if (all.length) flash(`${all.map((e) => `"${e}"`).join(", ")} added to the word list`);
  }

  // What Correct started, under the header until dismissed as its run goes on.
  let note = $state<string | null>(null);
  function tellFixed(r: api.Fixed) {
    tellLearned(r.learned);
  }

  // Fixes a re-transcription could not put on any word: listed once it ends, and on opening, until
  // dismissed, so a fix is never lost without the user seeing it.
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
    // Only what was shown: a run may have set more aside since, which stay to be seen.
    const seen = setAside;
    setAside = [];
    await call(() => api.forgetFixesSetAside(seen));
  }
  // A set-aside fix, or a word time set by moving a cut's edge, as the line lists it.
  function asideText(s: api.SetAside): string {
    const silent = s.silent ? ", its track has no words this time" : "";
    if (s.retimed) {
      return `the time of "${s.text}" (${stamp(s.at * 1000)}, set to ${stamp(s.retimed[0] * 1000)} to ${stamp(s.retimed[1] * 1000)}${silent})`;
    }
    return `"${s.text}" (was "${s.heard}", ${stamp(s.at * 1000)}${silent})`;
  }
  // The set-aside line: the fixes, and what Correct could not do (`note`), which the run's progress
  // line no longer shows by its end.
  const asideLine = $derived.by(() => {
    const fixes = setAside.filter((s) => !s.note);
    const notes = setAside.flatMap((s) => (s.note ? [s.note] : []));
    const listed = fixes.length
      ? `Set aside, not on any word now: ${fixes.map(asideText).join(", ")}. Fix those words again where they are now.`
      : "";
    return [listed, ...notes].filter((t) => t).join(" ");
  });
  async function correctAll() {
    const started = await correct();
    if (!started || !mounted) return;
    note = `Correcting: ${started.doing} (Properties shows how far).`;
    noted = true;
  }
  // The run the note is about: ended failed, the note says why instead of that it goes on (a
  // run asked to name the speakers afresh that could not hear them stops, and says so only here
  // and in Properties).
  let noted = $state(false);
  $effect(() => {
    if (!noted || analysing) return;
    noted = false;
    if (failed) note = `Correct stopped: ${error ?? "Properties says why"}.`;
  });

  const first = $derived(Math.max(0, Math.floor(top / ROW) - OVERSCAN));
  const visible = $derived.by(() => {
    if (!laid) return [];
    const last = Math.min(laid.rows.length, Math.ceil((top + height) / ROW) + OVERSCAN);
    const out = [];
    for (let i = first; i < last; i++) out.push({ index: i, row: laid.rows[i] });
    return out;
  });

  const current = $derived(words && player?.position != null ? wordAt(words, player.position * 1000) : -1);
  // The span the playhead is in, lit whole.
  const currentSpan = $derived(words && current >= 0 ? (words.span[current] ?? 0) : 0);
  // The selected words, sorted and joined, for drawing; the latest run, for Shift+click.
  const joined = $derived(sel.runs(selection));
  const latest = $derived(sel.latest(selection));

  // Flagged words: per edit or threshold, the kept ones to check, in order.
  const toCheck = $derived(toks && words ? flagged(toks, words.sure, flagBelow) : []);
  const doubt = (i: number) => flagBelow > 0 && words != null && words.sure[i] < flagBelow;
  // The one word selected (a single stretch of one word), when it is flagged: what "Looks right"
  // confirms.
  const checking = $derived(
    joined.length === 1 && joined[0].first === joined[0].last && doubt(joined[0].first) ? joined[0].first : -1,
  );

  // Next or previous flagged word from the selection, else from the playhead's word: selected,
  // played from and scrolled to. No wrapping: the end of the list stays an end.
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
  // The list of the recording's fixes (5ae), shown in place of the words while open; its jump
  // closes it and goes to the place as the flagged words' steps do.
  let listing = $state(false);
  // Closed whenever it cannot show (review, no words, a list not editable), or it came back in
  // place of the words, the word bar hidden, once review ended.
  $effect(() => {
    if (reviewing || !words || !edit?.editable) listing = false;
  });
  // The words hidden under the list lose their scroll position (a hidden element has none) while
  // `top` keeps it: put back when the list closes, or the rows drawn for `top` sit out of view.
  const away = $derived(listing && !!words && !!edit?.editable && !reviewing);
  let wasAway = false;
  $effect(() => {
    if (wasAway && !away && scroller) {
      ours = Math.round(top);
      scroller.scrollTop = top;
    }
    wasAway = away;
  });
  async function jumpTo(first: number, last: number) {
    if (!words) return;
    listing = false;
    await tick();
    goTo(first);
    if (last > first) onSelect(sel.words(first, last));
  }

  // Confirms the selected flagged word (it keeps what Whisper heard) and goes on to the next; with
  // none after it the header says so and the selection goes, so the end is seen.
  function looksRight() {
    const i = checking;
    if (i < 0) return;
    const next = stepFrom(toCheck, i, false);
    const left = toCheck.length - 1;
    call(async () => {
      await api.confirmWord(i);
      if (next >= 0) {
        goTo(next);
        return;
      }
      onSelect(null);
      flash(left > 0 ? "No flagged word after this one: ‹ goes back" : "Every flagged word checked");
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

  // F2: the field a double-click gives the word under the pointer, else Fix text for one stretch. A
  // window listener, so it works after a click on the picture; the word is looked up when F2 comes,
  // since the list can scroll under a still pointer without `:hover` noticing.
  let pointer: { x: number; y: number } | null = null;
  function typing(t: EventTarget | null): boolean {
    return (
      t instanceof HTMLInputElement ||
      t instanceof HTMLSelectElement ||
      t instanceof HTMLTextAreaElement ||
      (t instanceof HTMLElement && t.isContentEditable)
    );
  }
  function onF2(e: KeyboardEvent) {
    if (e.key !== "F2" || e.repeat || e.ctrlKey || e.altKey || e.shiftKey || e.metaKey) return;
    // A field open or being typed in keeps its keys, and a dialog in the page holds them; native
    // dialogs keep the keyboard from the page themselves.
    if (fixing != null || retyping || renaming || typing(e.target) || typing(document.activeElement)) return;
    if (document.querySelector('[aria-modal="true"]')) return;
    // The Transcript tab hidden (another tab, the panel closed): nothing of it to act on.
    if (!words || !scroller?.checkVisibility()) return;
    const i = pointer ? keptWordAt(pointer.x, pointer.y) : -1;
    if (i >= 0) {
      e.preventDefault();
      fixAt(i, selection);
    } else if (
      selection?.kind === "words" &&
      edit?.editable &&
      !reviewing &&
      sel.pairs(selection).length === 1 &&
      inSight(sel.runs(selection)[0])
    ) {
      // The word bar shows its actions with Fix text enabled (one stretch selected). Only for a
      // selection in sight (5ae): a click on no word leaves the selection where it was, and F2
      // opened Fix text holding a sentence fixed long before, for words out of view.
      e.preventDefault();
      retyping = true;
    }
  }

  /** Whether a word of `r` is in a row on screen (5ae). */
  function inSight(r: sel.Run): boolean {
    if (!laid) return false;
    const from = Math.floor(top / ROW);
    const to = Math.ceil((top + height) / ROW) - 1;
    for (let i = r.first; i <= r.last && i < laid.rowOf.length; i++) {
      const row = laid.rowOf[i];
      if (row >= from && row <= to) return true;
    }
    return false;
  }

  async function call(f: () => Promise<unknown>) {
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  }

  // Selecting: press, drag, release; a press and release without moving seeks. Shift extends the
  // latest stretch; Ctrl (Cmd on macOS) adds a stretch or takes out a word, without seeking. `base`
  // is what the drag's run goes beside: nothing, every stretch (Ctrl), all but the latest (Shift).
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

  // Selecting a run that reaches into a span takes the whole span.
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
    // first row or the last three away from under the pointer before a double-click's second press.
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
      // A span is taken out whole.
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
      // Hidden (the list of fixes in its place, the tab away): no size to lay the words out for.
      if (scroller.clientWidth === 0 && scroller.clientHeight === 0) return;
      height = scroller.clientHeight;
      clearTimeout(timer);
      // The first size at once, later ones once the resize settles.
      if (width === 0) width = scroller.clientWidth;
      else timer = window.setTimeout(() => (width = scroller.clientWidth), RESIZE_SETTLE_MS);
    });
    ro.observe(scroller);
    const at = (e: PointerEvent) => (pointer = { x: e.clientX, y: e.clientY });
    const gone = () => (pointer = null);
    window.addEventListener("pointermove", at, { capture: true, passive: true });
    document.documentElement.addEventListener("pointerleave", gone);
    window.addEventListener("keydown", onF2);
    return () => {
      window.removeEventListener("pointermove", at, { capture: true });
      document.documentElement.removeEventListener("pointerleave", gone);
      window.removeEventListener("keydown", onF2);
      clearTimeout(timer);
      mounted = false;
      clearTimeout(learnedTimer);
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
      <span class="muted learned" role="status">{learned}</span>
    {:else if words}
      <span class="muted">{words.text.length.toLocaleString()} words</span>
    {/if}
    {#if words && edit?.editable && !reviewing}
      <button
        class="list-fixes"
        class:on={listing}
        onclick={() => {
          // A field open goes as leaving it does (what was typed is saved), not back with the words.
          if (!listing) {
            retyping = false;
            if (fixing != null) finishFix(true);
          }
          listing = !listing;
        }}
        title={listing ? "Back to the transcript" : "Every fix of this recording, by text: go to one, or take wrong ones back"}
        >Fixes</button
      >
    {/if}
  </header>
  {#if listing && words && edit?.editable && !reviewing}
    <FixesList {edit} {words} onJump={(a, b) => void jumpTo(a, b)} onClose={() => (listing = false)} {onError} />
  {/if}
  {#if setAside.length}
    <div class="notice" role="status">
      <span class="grow"
        >{asideLine}</span
      >
      <button class="x" onclick={() => void forgetSetAside()} aria-label="Dismiss" title="Seen: stop listing them">×</button>
    </div>
  {/if}
  {#if note}
    <div class="notice" role="status">
      <span class="grow">{note}</span>
      <button class="x" onclick={() => (note = null)} aria-label="Dismiss">×</button>
    </div>
  {/if}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div
    class="scroll"
    class:away={away}
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
              : "No transcript for this recording yet. Correct analyses it again, to edit by text."}
      </p>
      {#if !analysing && !loading}
        <p class="note"><button class="correct" onclick={() => void correctAll()}>Correct…</button></p>
      {/if}
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
                      ? `Whisper was ${words.sure[t.i]} % sure of this word: double-click or press F2 to fix it, or select it and "Looks right" below`
                      : "Double-click or press F2 to fix the spelling"}
                  data-i={t.i}>{words.text[t.i]}</span
                >
              {:else if t.kind === "spanword"}
                <span
                  class="w fixed"
                  class:cur={currentSpan === t.s}
                  class:sel={sel.has(joined, t.i)}
                  title="Fixed as one text: double-click or press F2 to change it"
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
  <!-- Kept while nothing is selected: a bar that came with the selection shrank the scroller
       between a double-click's two presses. -->
  {#if words && edit?.editable && !reviewing && !listing}
    <WordBar
      {edit}
      {words}
      selection={selection?.kind === "words" ? selection : null}
      {onSelect}
      {onError}
      bind:retyping
      checks={toCheck.length > 0}
      onFixed={tellFixed}
      onCorrect={correctAll}
      {analysing}
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
  .notice {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 12px;
    font-size: 12px;
    color: var(--muted);
    border-bottom: 1px solid var(--line);
  }
  .notice .grow {
    flex: 1;
    min-width: 0;
  }
  .notice button {
    padding: 2px 8px;
    font-size: 12px;
  }
  .scroll {
    position: relative;
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
  }
  .scroll.away {
    display: none;
  }
  .list-fixes {
    margin-left: 8px;
    padding: 0 8px;
    font-size: 12px;
    font-weight: 400;
    line-height: 18px;
  }
  .list-fixes.on {
    color: var(--on-accent);
    background: var(--accent);
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
    background: color-mix(in srgb, var(--accent) 28%, transparent);
  }
  .w.fixed {
    text-decoration: underline dotted rgba(255, 255, 255, 0.35);
    text-underline-offset: 3px;
  }
  .w.cur {
    color: var(--on-accent);
    background: var(--accent);
  }
  .w.doubt {
    text-decoration: underline wavy color-mix(in srgb, var(--accent) 90%, transparent);
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
    background: color-mix(in srgb, var(--bad) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--bad) 35%, transparent);
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
    background: color-mix(in srgb, var(--bad) 45%, transparent);
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
  /* The empty transcript's way to analyse again, as the word bar's Correct looks. */
  .note .correct {
    padding: 4px 14px;
    font-weight: 600;
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
