// The caption settings' groups (2t): each `CaptionRules` part's heading and (i), in the order both
// the Captions panel and Settings' caption defaults show them, so the two screens cannot drift.
// The panel puts its own sections (Speakers, Export, Remembered voices) between these.

export type CaptionPart = "switches" | "style" | "motion" | "overlap" | "text" | "cut" | "resolve";

export const CAPTION_SECTIONS: { part: CaptionPart; title: string; info?: string }[] = [
  { part: "switches", title: "Export and preview" },
  {
    part: "style",
    title: "Style",
    info: "The preview and Send to Resolve use all of these. An SRT carries only the colour, bold, italic and underline: in Resolve the rest is the subtitle track's style (Inspector > Track). If a Track Style change misses bold or italic SRT captions, turn those off.",
  },
  {
    part: "motion",
    title: "Highlight and animation",
    info: "Resolve and the preview only; the SRT ignores them. Each caption then becomes an animated caption, a heavier Fusion clip than a plain Text+: thousands of them on an hour-long recording can slow Resolve down, so both are off by default. They need Junctur's own template.",
  },
  {
    part: "overlap",
    title: "People talking at once",
    info: "Where their captions go in the preview and in Resolve. Side by side needs short lines (about 20 characters). The SRT always stacks them in one caption, first to speak on top.",
  },
  {
    part: "text",
    title: "Caption text",
    info: "What the captions show in the SRT, in Resolve and in the preview; the transcript keeps the words as spoken. A translation comes first, then find and replace, case, punctuation and censoring.",
  },
  { part: "cut", title: "How captions are cut" },
  {
    part: "resolve",
    title: "Send to Resolve",
    info: "Sends the whole edit to the project open in Resolve: a new timeline named after the recording, cut as here with every audio track, and every caption on it as a Text+ clip, a video track per speaker. If the project already has that timeline, Junctur asks whether to make a new one beside it or replace it. Once per Resolve session run Workspace > Scripts > Junctur and leave it running.",
  },
];

/** One group's heading and (i), by part. */
export const section = (part: CaptionPart) => CAPTION_SECTIONS.find((s) => s.part === part)!;
