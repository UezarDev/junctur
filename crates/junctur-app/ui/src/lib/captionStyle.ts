/**
 * How a caption looks in the editor's preview over the picture (5d): the CSS for the caption
 * settings (the panel's sample lines take the same weight and slant).
 *
 * Only bold and italic are the user's here, because they are the only styles an SRT carries into
 * DaVinci Resolve (5d's research). The rest imitates Resolve's default subtitle Track Style, which
 * is where size, font, outline, shadow and position are set for the whole track: Open Sans
 * Semibold (Resolve's default face, which its own SRT export marks as bold), white unless a speaker
 * colour says otherwise, near the bottom. It is an approximation of Resolve's default, not a
 * promise of what any Track Style will show; a dark shadow keeps it readable over any picture.
 */
import type { Captions } from "./api";

/** Caption text height over picture height, near Resolve's default subtitle size on 1080p. */
const SIZE = 0.045;

/** Inline CSS for a caption block over a picture `pictureHeight` pixels tall. */
export function captionStyle(c: Pick<Captions, "bold" | "italic">, pictureHeight: number): string {
  const px = Math.max(10, Math.round(pictureHeight * SIZE));
  return [
    `font-family: "Open Sans", "Segoe UI", system-ui, sans-serif`,
    `font-size: ${px}px`,
    `line-height: 1.2`,
    `font-weight: ${c.bold ? 700 : 600}`,
    `font-style: ${c.italic ? "italic" : "normal"}`,
    `text-shadow: 0 0 3px #000, 0 1px 2px #000`,
    `margin-bottom: ${Math.round(pictureHeight * 0.06)}px`,
  ].join("; ");
}
