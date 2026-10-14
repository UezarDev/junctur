// The caption settings' groups (2t): each `CaptionRules` part's heading and (i), in the order both
// the Captions panel and Settings' caption defaults show them, so the two screens cannot drift.
// The panel puts its own sections (Speakers, Export, Remembered voices) between these.

export type CaptionPart = "switches" | "style" | "overlap" | "text" | "cut" | "resolve";

export const CAPTION_SECTIONS: { part: CaptionPart; title: string; info?: string }[] = [
  { part: "switches", title: "Export and preview" },
  {
    part: "style",
    title: "Style",
    info: "The preview and Send to Resolve use all of these. An SRT carries only the colour, bold, italic and underline: in Resolve the rest is the subtitle track's style (Inspector > Track). If a Track Style change misses bold or italic SRT captions, turn those off.",
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
    info: "Places every caption on the open Resolve timeline as a Text+ clip, a video track per speaker, at the edited timeline's times: put the edited video at the timeline's start. Once per Resolve session run Workspace > Scripts > Junctur and leave it running. Sending again updates the captions already there.",
  },
];

/** One group's heading and (i), by part. */
export const section = (part: CaptionPart) => CAPTION_SECTIONS.find((s) => s.part === part)!;
