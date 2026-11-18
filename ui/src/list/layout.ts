// Row geometry for the message list. Pure functions, no DOM.
//
// Every row's height is known before any row is loaded: the core supplies each
// row's preview length, and the number of preview lines (0 to 3) is estimated
// from that and the list's width. The row is then drawn at exactly that
// height, so nothing ever needs measuring and the scrollbar never jumps.
//
// The estimate is by character count, so a preview occasionally gets a line
// more than it needs. Milestone 3 can replace `linesFor` with real text
// measurement without touching anything else.

/** Height of one line of text, px. Must match --line in styles.css. */
export const LINE = 18;
/** Vertical padding (8 + 8), sender line, subject line, bottom border. */
export const ROW_FIXED = 16 + LINE + LINE + 1;
export const MAX_PREVIEW_LINES = 3;
/** Horizontal padding inside a row (14 + 14). Must match .row in styles.css. */
const ROW_PAD_X = 28;
/** Average glyph width of Inter at 13px, rounded up so estimates err tall. */
const AVG_CHAR = 6.5;

export interface Layout {
  /** Preview lines per row. */
  lines: Uint8Array;
  /** offsets[i] is the top of row i; offsets[count] is the total height. */
  offsets: Float64Array;
}

export function charsPerLine(listWidth: number): number {
  return Math.max(16, Math.floor((listWidth - ROW_PAD_X) / AVG_CHAR));
}

export function linesFor(previewLen: number, perLine: number): number {
  if (previewLen <= 0) return 0;
  return Math.min(MAX_PREVIEW_LINES, Math.ceil(previewLen / perLine));
}

export function buildLayout(
  previewLens: ArrayLike<number>,
  listWidth: number,
  /** Extra height for particular rows: an expanded conversation's tree. */
  extra?: ReadonlyMap<number, number>,
): Layout {
  const count = previewLens.length;
  const perLine = charsPerLine(listWidth);
  const lines = new Uint8Array(count);
  const offsets = new Float64Array(count + 1);
  let y = 0;
  for (let i = 0; i < count; i++) {
    const n = linesFor(previewLens[i]!, perLine);
    lines[i] = n;
    offsets[i] = y;
    y += ROW_FIXED + n * LINE + (extra?.get(i) ?? 0);
  }
  offsets[count] = y;
  return { lines, offsets };
}

/** The row that contains vertical position `y`, clamped to the valid range. */
export function rowAt(layout: Layout, y: number): number {
  const count = layout.lines.length;
  if (count === 0) return -1;
  const offsets = layout.offsets;
  let lo = 0;
  let hi = count - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >>> 1;
    if (offsets[mid]! <= y) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}
