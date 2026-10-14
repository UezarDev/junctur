// The page is never reloaded by hand (2q). WebView2's browser keys (F5, Ctrl+R, Ctrl+Shift+R) and
// its default context menu's Refresh reloaded it: the app went on (mpv playing, the recording open)
// while the page started over, and before 2q it came back with an empty Transcript and Properties.
// A page that reloads anyway (a dev build's Vite, a crash) reattaches to the open recording
// (App.svelte `reattach`). Turning WebView2's browser keys off instead is not reachable from Tauri
// 2.12, and would take F12 / Ctrl+Shift+I, the dev build's DevTools, with them; those stay.

/** A key that reloads the page in WebView2: F5 with any modifier, Ctrl (Cmd) + R with or without
 * Shift, and a keyboard's own Refresh key. */
export function reloads(e: Pick<KeyboardEvent, "code" | "key" | "ctrlKey" | "metaKey" | "altKey">): boolean {
  if (e.code === "F5" || e.key === "F5" || e.code === "BrowserRefresh" || e.key === "BrowserRefresh") return true;
  return (e.ctrlKey || e.metaKey) && !e.altKey && e.code === "KeyR";
}

/** A text field, where the context menu is Cut, Copy and Paste rather than Back and Refresh. */
function editable(t: EventTarget | null): boolean {
  return t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || (t instanceof HTMLElement && t.isContentEditable);
}

/** Cancels the reload keys everywhere in the window, before any element's own handler (capture: a
 * focused field, the Settings dialog or review cannot hold one back), and the default context menu
 * outside text fields in a release build (a dev build keeps it for Inspect). Returns the undo. */
export function blockReload(): () => void {
  const key = (e: KeyboardEvent) => {
    if (reloads(e)) e.preventDefault();
  };
  const menu = (e: MouseEvent) => {
    if (!import.meta.env.DEV && !editable(e.target)) e.preventDefault();
  };
  window.addEventListener("keydown", key, true);
  window.addEventListener("contextmenu", menu, true);
  return () => {
    window.removeEventListener("keydown", key, true);
    window.removeEventListener("contextmenu", menu, true);
  };
}
