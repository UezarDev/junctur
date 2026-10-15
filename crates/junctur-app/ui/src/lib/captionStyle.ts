/**
 * The caption settings as CSS, for the preview over the picture and the Captions panel's samples.
 * Where each speaker's block sits is the engine's (`export/layout.rs`), the placement Resolve gets.
 */
import type { Captions } from "./api";

type Styled = Pick<Captions, "bold" | "italic" | "underline">;

/** Line height over the font size, as the engine's layout counts it (`layout::LINE`). */
export const LINE = 1.2;

/** The weight, slant and underline the settings give: Text+ "Regular" or "Bold". */
export function captionFace(c: Styled): { weight: number; style: string; decoration: string } {
  return {
    weight: c.bold ? 700 : 400,
    style: c.italic ? "italic" : "normal",
    decoration: c.underline ? "underline" : "none",
  };
}

/** The font stack: the chosen font, then fallbacks for a machine without it. */
export function captionFont(c: Pick<Captions, "font">): string {
  const name = c.font.replace(/["\\]/g, "").trim();
  return `"${name}", "Open Sans", "Segoe UI", system-ui, sans-serif`;
}

/** The outline as a ring of text shadows (a browser has no outside-only stroke for HTML text),
 * about as thick as Text+ draws `outlineWidth` on text `fontPx` high; a soft shadow when off, so
 * the preview stays readable over any picture. */
export function captionOutline(
  c: Pick<Captions, "outline" | "outlineWidth" | "outlineColor">,
  fontPx: number,
): string {
  if (!c.outline) return "0 0 3px #000, 0 1px 2px #000";
  const r = Math.max(1, c.outlineWidth * 0.02 * fontPx);
  const ring: string[] = [];
  for (let i = 0; i < 16; i++) {
    const a = (i / 16) * 2 * Math.PI;
    ring.push(`${(Math.cos(a) * r).toFixed(1)}px ${(Math.sin(a) * r).toFixed(1)}px 0 ${c.outlineColor}`);
  }
  return ring.join(", ");
}

/** Inline CSS for every caption block over a picture `pictureHeight` pixels tall. Not the
 * underline: a decoration takes the colour of the box that sets it, so each line sets its own
 * (`captionFace`) to be underlined in its speaker's colour. */
export function captionStyle(c: Captions, pictureHeight: number): string {
  const px = Math.max(6, Math.round((pictureHeight * c.size) / 100));
  const face = captionFace(c);
  return [
    `font-family: ${captionFont(c)}`,
    `font-size: ${px}px`,
    `line-height: ${LINE}`,
    `font-weight: ${face.weight}`,
    `font-style: ${face.style}`,
    `text-shadow: ${captionOutline(c, px)}`,
  ].join("; ");
}

/** A speaker's colours under the word highlight, as the engine gives Resolve
 * (`Captions::highlight_colors`): the text's and the lit word's. A speaker with no colour (or
 * white) gets a yellow word. */
export function highlightColors(c: Pick<Captions, "highlightColor">, speaker: string | undefined): [string, string] {
  const valid = (x: string | undefined) => (x && /^#[0-9a-fA-F]{6}$/.test(x) ? x.toUpperCase() : null);
  const own = valid(speaker) ?? "#FFFFFF";
  const chosen = valid(c.highlightColor);
  if (chosen) return [own, chosen];
  return own === "#FFFFFF" ? ["#FFFFFF", "#FFD400"] : ["#FFFFFF", own];
}

/** How far a caption is into its in or out animation at output second `t`, 0 hidden to 1
 * whole, as the animated caption plays it in Resolve: in over `animationLength` after its start, out
 * over the same before its end. */
export function animationProgress(
  c: Pick<Captions, "animation" | "animationLength">,
  start: number,
  end: number,
  t: number,
): number {
  if (c.animation === "none" || c.animationLength <= 0) return 1;
  const p = Math.min(t - start, end - t) / c.animationLength;
  return Math.max(0, Math.min(1, p));
}

/** The CSS a caption block gets at animation progress `p`, on top of its centring. */
export function animationStyle(c: Pick<Captions, "animation">, p: number): { opacity: string; transform: string } {
  const centre = "translate(-50%, -50%)";
  if (c.animation === "none" || p >= 1) return { opacity: "1", transform: centre };
  const grow = c.animation === "pop" ? ` scale(${(0.6 + 0.4 * p).toFixed(3)})` : "";
  const rise = c.animation === "slide" ? ` translateY(${((1 - p) * 0.6).toFixed(3)}em)` : "";
  return { opacity: p.toFixed(3), transform: centre + rise + grow };
}
