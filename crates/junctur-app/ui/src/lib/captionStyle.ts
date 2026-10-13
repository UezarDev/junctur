/**
 * How a caption looks in the editor's preview over the picture (5d) and in the Captions panel's
 * speaker samples: the caption settings as CSS.
 *
 * Since 5g the look is the user's (font, size, position, outline, bold, italic, underline) and is
 * what the Text+ clips sent to DaVinci Resolve get, so the preview shows those. Where each
 * speaker's block sits is not decided here but by the engine (`export/layout.rs`), the same
 * placement Resolve gets. An SRT still carries only colour, bold, italic and underline (5d, 5f);
 * imported that way, Resolve's subtitle Track Style decides the rest.
 */
import type { Captions } from "./api";

type Styled = Pick<Captions, "bold" | "italic" | "underline">;

/** Line height over the font size, as the engine's layout counts it (`layout::LINE`). */
export const LINE = 1.2;

/** The weight, slant and underline the settings give: Text+ "Regular" or "Bold" (5g; before, 5d
 * imitated Resolve's Semibold subtitle face). */
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
