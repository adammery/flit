// File-type presentation for attachment chips in the conversation view, and
// which attachments were saved this run.

import { SvelteSet } from "svelte/reactivity";

/** Ids of the attachments saved this run — mirrors what the backend
 * remembers, so their chips offer Open and Show in Finder. App-wide, so a
 * conversation opened again still offers them. */
export const savedAttachments = new SvelteSet<number>();

/** Uppercased extension for the type tile, at most 4 characters so it fits;
 * empty when the filename has none (or is all extension, like dotfiles). */
export function fileExt(name: string): string {
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) return "";
  return name.slice(dot + 1).toUpperCase().slice(0, 4);
}

/** Tile colors per family: documents blue, sheets green, decks orange,
 * archives gray, images purple — red is reserved for PDF. */
const EXT_COLORS: Record<string, string> = {
  PDF: "#e0443e",
  DOC: "#0a66c2",
  DOCX: "#0a66c2",
  PAGE: "#0a66c2",
  TXT: "#0a66c2",
  XLS: "#34c759",
  XLSX: "#34c759",
  CSV: "#34c759",
  NUMB: "#34c759",
  PPT: "#e08a3e",
  PPTX: "#e08a3e",
  KEY: "#e08a3e",
  ZIP: "#8e8e93",
  RAR: "#8e8e93",
  GZ: "#8e8e93",
  "7Z": "#8e8e93",
  PNG: "#7a5cc2",
  JPG: "#7a5cc2",
  JPEG: "#7a5cc2",
  GIF: "#7a5cc2",
  HEIC: "#7a5cc2",
  WEBP: "#7a5cc2",
  SVG: "#7a5cc2",
};

export function extColor(ext: string): string {
  return EXT_COLORS[ext] ?? "#8e8e93";
}
