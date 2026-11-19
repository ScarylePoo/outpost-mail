// How the app looks: colour scheme, accent colour, fonts and sizes.
//
// The choices are kept by the core (settings.json). A copy is kept in the
// webview's own storage as well, only so that the window can be drawn in the
// right colours from its very first frame, before the core has answered.
//
// Everything is applied to the document root: the scheme and accent as
// attributes that styles.css keys its colours on, the fonts as custom
// properties, and the size as the window's zoom, so every measurement in the
// interface (the message list computes its own row heights) scales together.

import { createSignal } from "solid-js";
import { core } from "./index";
import type { Appearance } from "./api";

export const SCHEMES: { id: string; name: string; hint: string; dark: boolean; swatch: [string, string, string] }[] = [
  { id: "system", name: "Match the system", hint: "Light or dark, following your desktop", dark: false, swatch: ["#ffffff", "#23262b", "#1f5fbf"] },
  { id: "light", name: "Light", hint: "", dark: false, swatch: ["#ffffff", "#f1f2f4", "#1c1f23"] },
  { id: "dark", name: "Dark", hint: "", dark: true, swatch: ["#23262b", "#1b1d21", "#e6e8eb"] },
  { id: "paper", name: "Paper", hint: "Warm and low-glare", dark: false, swatch: ["#fbf7ee", "#efe9dc", "#2b2620"] },
  { id: "slate", name: "Slate", hint: "Dark with a blue cast", dark: true, swatch: ["#1f2937", "#18202b", "#e5eaf1"] },
  { id: "contrast", name: "High contrast", hint: "Black on white, strong lines", dark: false, swatch: ["#ffffff", "#ffffff", "#000000"] },
];

export const ACCENTS: { id: string; name: string; color: string }[] = [
  { id: "blue", name: "Blue", color: "#1f5fbf" },
  { id: "teal", name: "Teal", color: "#0b7285" },
  { id: "green", name: "Green", color: "#1a7f37" },
  { id: "purple", name: "Purple", color: "#6b3fbf" },
  { id: "rose", name: "Rose", color: "#b8336a" },
  { id: "orange", name: "Orange", color: "#b3540a" },
  { id: "graphite", name: "Graphite", color: "#3f4753" },
];

export const TEXT_SIZES = [80, 90, 100, 110, 125, 150, 175, 200];
export const READING_SIZES = [11, 12, 13, 14, 15, 16, 18, 20, 22, 24, 28];

export const DEFAULT_APPEARANCE: Appearance = {
  scheme: "system",
  accent: "blue",
  font: "",
  textSize: 100,
  readingFont: "",
  readingSize: 14,
  writingFont: "",
};

const KEPT = "outpost.appearance";
const BUILT_IN = `"Inter Variable", "Inter", system-ui, sans-serif`;
const SYSTEM = `system-ui, -apple-system, "Segoe UI", sans-serif`;

function kept(): Appearance {
  try {
    const text = localStorage.getItem(KEPT);
    if (text) return { ...DEFAULT_APPEARANCE, ...(JSON.parse(text) as Partial<Appearance>) };
  } catch {
    // No storage, or nonsense in it: the defaults will do until the core answers.
  }
  return DEFAULT_APPEARANCE;
}

const [appearance, setAppearanceSignal] = createSignal<Appearance>(kept());
/** Average width of a character of the interface font at list size, px. */
const [charWidth, setCharWidth] = createSignal(6.5);
export { appearance, charWidth };

/** A font name as it may appear in a style sheet. The core cleans names too. */
function safe(name: string): string {
  return name.replace(/[^\p{L}\p{N} ._-]/gu, "").trim();
}

function family(name: string, fallback: string): string {
  const clean = safe(name);
  return clean ? `"${clean}", ${fallback}` : fallback;
}

/** The font messages are read in, as a CSS font-family value. */
export function readingFamily(): string {
  return family(appearance().readingFont, SYSTEM);
}

const systemDark = window.matchMedia?.("(prefers-color-scheme: dark)");

/** The scheme in use once "system" has been resolved. */
export function schemeInUse(a: Appearance = appearance()): string {
  if (a.scheme === "system") return systemDark?.matches ? "dark" : "light";
  return SCHEMES.some((s) => s.id === a.scheme) ? a.scheme : "light";
}

let canvas: CanvasRenderingContext2D | null | undefined;
function measure(font: string, text: string): number {
  if (canvas === undefined) canvas = document.createElement("canvas").getContext("2d");
  if (!canvas) return 0;
  canvas.font = font;
  return canvas.measureText(text).width;
}

const SAMPLE = "The quick brown fox jumps over the lazy dog, and 1,234 others did not.";

let zoomed = 1;

function apply(a: Appearance) {
  const root = document.documentElement;
  const scheme = schemeInUse(a);
  root.dataset.scheme = scheme;
  root.dataset.dark = String(SCHEMES.find((s) => s.id === scheme)?.dark ?? false);
  root.dataset.accent = a.accent;
  root.style.setProperty("--font", family(a.font, BUILT_IN));
  root.style.setProperty("--read-font", family(a.readingFont, SYSTEM));
  root.style.setProperty("--read-size", `${a.readingSize}px`);
  root.style.setProperty("--write-font", family(a.writingFont || a.readingFont, SYSTEM));
  const factor = Math.min(2, Math.max(0.8, a.textSize / 100));
  if (factor !== zoomed) {
    zoomed = factor;
    void core.setZoom(factor).catch(() => {});
  }
  // The message list decides how many lines a preview needs from the
  // average width of a character, so it has to know the font's.
  const measureNow = () => {
    const width = measure(`13px ${family(a.font, BUILT_IN)}`, SAMPLE);
    if (width > 0) setCharWidth(Math.max(4, Math.min(12, (width / SAMPLE.length) * 1.06)));
  };
  measureNow();
  // The built-in font may still be loading; measure again once it is in.
  void document.fonts?.ready.then(measureNow);
}

apply(appearance());
systemDark?.addEventListener?.("change", () => {
  apply(appearance());
  setAppearanceSignal({ ...appearance() });
});

function keep(a: Appearance) {
  setAppearanceSignal(a);
  apply(a);
  try {
    localStorage.setItem(KEPT, JSON.stringify(a));
  } catch {
    // Only the first-frame copy is lost.
  }
}

export function loadAppearance(): Promise<void> {
  return core
    .send({ type: "getAppearance" })
    .then((r) => keep(r.appearance))
    .catch(() => {});
}

/** Shows the change at once and then has the core keep it. */
export function saveAppearance(next: Appearance): Promise<void> {
  keep(next);
  return core.send({ type: "setAppearance", appearance: next }).then((r) => keep(r.appearance));
}

/** Fonts worth offering, where the computer has them. */
const CANDIDATES = [
  "Segoe UI", "Segoe UI Variable", "Aptos", "Calibri", "Candara", "Corbel", "Arial", "Helvetica", "Helvetica Neue",
  "Verdana", "Tahoma", "Trebuchet MS", "Century Gothic", "Franklin Gothic Medium", "Lucida Sans Unicode",
  "Noto Sans", "Open Sans", "Roboto", "Ubuntu", "Cantarell", "DejaVu Sans", "Liberation Sans", "Source Sans 3",
  "Fira Sans", "Lato", "IBM Plex Sans", "Atkinson Hyperlegible", "OpenDyslexic", "Comic Sans MS",
  "Georgia", "Cambria", "Constantia", "Times New Roman", "Palatino Linotype", "Book Antiqua", "Garamond",
  "Noto Serif", "DejaVu Serif", "Liberation Serif", "Source Serif 4", "Charter", "Sitka Text",
  "Consolas", "Cascadia Mono", "Cascadia Code", "Courier New", "Lucida Console", "DejaVu Sans Mono",
  "Liberation Mono", "Ubuntu Mono", "Fira Code", "JetBrains Mono", "Source Code Pro", "Noto Sans Mono",
];

/** Whether the computer has a font: text set in it measures differently
 *  from the same text in at least one of the generic fallbacks. */
export function fontInstalled(name: string): boolean {
  const clean = safe(name);
  if (!clean) return false;
  return ["monospace", "serif", "sans-serif"].some(
    (generic) => measure(`40px "${clean}", ${generic}`, SAMPLE) !== measure(`40px ${generic}`, SAMPLE),
  );
}

let found: string[] | undefined;
export function installedFonts(): string[] {
  found ??= CANDIDATES.filter(fontInstalled);
  return found;
}
