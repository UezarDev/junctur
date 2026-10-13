/**
 * How a caption looks in the editor's preview over the picture (5d): the CSS for the caption
 * settings (the panel's sample lines take the same face, `captionFace`).
 *
 * Only bold, italic and underline are the user's here, because they are the only styles an SRT
 * carries into DaVinci Resolve (5d's research, 5f's underline test). The rest imitates Resolve's
 * default subtitle Track Style, which is where size, font, outline, shadow and position are set
 * for the whole track: Open Sans Semibold (Resolve's default face, which its own SRT export marks
 * as bold), white unless a speaker colour says otherwise, near the bottom. It is an approximation of Resolve's default, not a
 * promise of what any Track Style will show; a dark shadow keeps it readable over any picture.
 */
import type { Captions } from "./api";

/** Caption text height over picture height, near Resolve's default subtitle size on 1080p. */
const SIZE = 0.045;

type Styled = Pick<Captions, "bold" | "italic" | "underline">;

/** The weight, slant and underline the settings give: Resolve's default face is Semibold, bold is
 * Bold. Shared with the Captions panel's speaker samples. */
export function captionFace(c: Styled): { weight: number; style: string; decoration: string } {
  return {
    weight: c.bold ? 700 : 600,
    style: c.italic ? "italic" : "normal",
    decoration: c.underline ? "underline" : "none",
  };
}

/** Inline CSS for a caption block over a picture `pictureHeight` pixels tall. Not the underline: a
 * decoration takes the colour of the box that sets it, so each line sets its own (`captionFace`) to
 * be underlined in its speaker's colour, as Resolve shows it. */
export function captionStyle(c: Styled, pictureHeight: number): string {
  const px = Math.max(10, Math.round(pictureHeight * SIZE));
  const face = captionFace(c);
  return [
    `font-family: "Open Sans", "Segoe UI", system-ui, sans-serif`,
    `font-size: ${px}px`,
    `line-height: 1.2`,
    `font-weight: ${face.weight}`,
    `font-style: ${face.style}`,
    `text-shadow: 0 0 3px #000, 0 1px 2px #000`,
    `margin-bottom: ${Math.round(pictureHeight * 0.06)}px`,
  ].join("; ");
}
