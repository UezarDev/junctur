/**
 * The title bar's menus (2i): File and View, as 2g's native menu had them, plus Close recording. One
 * list of ids, so the bar, the keyboard and test mode's `menu=<id>` (src/testmode.rs) all end in
 * App.svelte's one `onMenu`. Check marks and what is enabled come from the page's own state.
 */

export type MenuId =
  | "open"
  | "close"
  | "settings"
  | "panel"
  | "talking"
  | "transcript"
  | "captions"
  | "properties";

export interface MenuItem {
  id: MenuId;
  label: string;
  /** Shown beside the label; the page's key handler does the work. */
  keys?: string;
  /** Has a check mark, set from `MenuState`. */
  check?: boolean;
  /** Needs a recording open. */
  needsFile?: boolean;
  /** A line above it. */
  separator?: boolean;
}

export interface Menu {
  id: "file" | "view";
  label: string;
  items: MenuItem[];
}

export const MENUS: Menu[] = [
  {
    id: "file",
    label: "File",
    items: [
      { id: "open", label: "Open recording…", keys: "Ctrl+O" },
      { id: "close", label: "Close recording", keys: "Ctrl+W", needsFile: true },
      { id: "settings", label: "Settings…", keys: "Ctrl+,", separator: true },
    ],
  },
  {
    id: "view",
    label: "View",
    items: [
      { id: "panel", label: "Side panel", keys: "T", check: true, needsFile: true },
      { id: "talking", label: "Talking now overlay", check: true },
      { id: "transcript", label: "Transcript", needsFile: true, separator: true },
      { id: "captions", label: "Captions", needsFile: true },
      { id: "properties", label: "Properties", needsFile: true },
    ],
  },
];

/** What the menus show checked and enabled. */
export interface MenuState {
  loaded: boolean;
  /** A recording is open or opening: Close recording works on both. */
  closable: boolean;
  panel: boolean;
  talking: boolean;
  /** A dialog is open (Settings is modal): the menus wait, the window buttons do not. */
  locked: boolean;
}

export function checked(item: MenuItem, s: MenuState): boolean {
  return item.id === "panel" ? s.panel : item.id === "talking" ? s.talking : false;
}

export function enabled(item: MenuItem, s: MenuState): boolean {
  if (item.id === "close") return s.closable;
  return !item.needsFile || s.loaded;
}
