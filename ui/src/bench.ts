// The milestone 0 shell test: drives the message list through a fixed scroll
// script and reports frame timing and blank rows. See docs/testing.md.
//
// What it can measure: how long the page takes to produce each frame, and
// whether any visible row was missing its data when a frame was produced.
// What it cannot measure: tearing, flicker or white flashes painted by the
// engine itself. Those need a person watching, which is why the test is
// "run this, and also scroll by hand".

import type { ListHandle } from "./list/MessageList";

interface Phase {
  name: string;
  seconds: number;
  /** Returns the scroll position for time `t` (seconds into the phase). */
  position(t: number, max: number): number;
}

const PHASES: Phase[] = [
  { name: "reading speed (600 px/s)", seconds: 4, position: (t) => t * 600 },
  { name: "fast wheel (6,000 px/s)", seconds: 4, position: (t) => 2_400 + t * 6_000 },
  { name: "scrollbar drag (120,000 px/s)", seconds: 4, position: (t) => 26_400 + t * 120_000 },
  {
    name: "back and forth",
    seconds: 4,
    position: (t, max) => max / 2 + Math.sin(t * 4) * 20_000,
  },
  {
    name: "random jumps (5 per second)",
    seconds: 4,
    position: (t, max) => {
      const step = Math.floor(t * 5);
      return ((step * 2_654_435_761) % 1_000_003) / 1_000_003 * max;
    },
  },
];

export interface PhaseResult {
  name: string;
  frames: number;
  median: number;
  p95: number;
  p99: number;
  worst: number;
  /** Frames that took more than 1.5 display intervals. */
  slow: number;
  /** Frames in which at least one visible row had no data. */
  blankFrames: number;
  /** Longest run of consecutive blank frames, ms. */
  longestBlank: number;
}

function quantile(sorted: number[], q: number): number {
  if (sorted.length === 0) return 0;
  return sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))]!;
}

function nextFrame(): Promise<number> {
  return new Promise((resolve) => requestAnimationFrame(resolve));
}

async function displayInterval(): Promise<number> {
  const deltas: number[] = [];
  let last = await nextFrame();
  for (let i = 0; i < 40; i++) {
    const now = await nextFrame();
    deltas.push(now - last);
    last = now;
  }
  deltas.sort((a, b) => a - b);
  return quantile(deltas, 0.5);
}

export async function runScrollTest(
  list: ListHandle,
  onProgress: (text: string) => void,
): Promise<string> {
  const scroller = list.scroller;
  const max = scroller.scrollHeight - scroller.clientHeight;
  scroller.scrollTop = 0;
  onProgress("Measuring the display…");
  const interval = await displayInterval();

  const results: PhaseResult[] = [];
  for (const phase of PHASES) {
    onProgress(`Scroll test: ${phase.name}…`);
    const deltas: number[] = [];
    let blankFrames = 0;
    let blankRun = 0;
    let longestBlank = 0;
    const start = await nextFrame();
    let last = start;
    for (;;) {
      const t = (last - start) / 1000;
      if (t >= phase.seconds) break;
      scroller.scrollTop = Math.max(0, Math.min(max, phase.position(t, max)));
      const now = await nextFrame();
      const delta = now - last;
      deltas.push(delta);
      // Sampled at the start of the next frame: this is what was just shown.
      if (list.blankVisible() > 0) {
        blankFrames++;
        blankRun += delta;
        longestBlank = Math.max(longestBlank, blankRun);
      } else {
        blankRun = 0;
      }
      last = now;
    }
    const sorted = [...deltas].sort((a, b) => a - b);
    results.push({
      name: phase.name,
      frames: deltas.length,
      median: quantile(sorted, 0.5),
      p95: quantile(sorted, 0.95),
      p99: quantile(sorted, 0.99),
      worst: sorted[sorted.length - 1] ?? 0,
      slow: deltas.filter((d) => d > interval * 1.5).length,
      blankFrames,
      longestBlank,
    });
  }
  // Can the very last message be reached? (It could not, in folders taller
  // than the engine's maximum element height, before the list was scaled.)
  scroller.scrollTop = scroller.scrollHeight;
  await nextFrame();
  await nextFrame();
  const lastSeen = list.visibleRange()[1];
  scroller.scrollTop = 0;
  await nextFrame();
  await nextFrame();
  const firstSeen = list.visibleRange()[0];

  const ms = (n: number) => n.toFixed(1).padStart(6);
  const lines = [
    "Outpost shell test",
    `Date:     ${new Date().toISOString()}`,
    `Engine:   ${navigator.userAgent}`,
    `Rows:     ${list.rowCount().toLocaleString("en")}   List height: ${Math.round(scroller.scrollHeight).toLocaleString("en")} px`,
    `Window:   ${window.innerWidth}x${window.innerHeight} @${window.devicePixelRatio}x   Display interval: ${interval.toFixed(1)} ms (${Math.round(1000 / interval)} Hz)`,
    "",
    "phase                            frames  median     p95     p99   worst   slow  blank  longest blank",
  ];
  for (const r of results) {
    lines.push(
      `${r.name.padEnd(32)} ${String(r.frames).padStart(6)}  ${ms(r.median)}  ${ms(r.p95)}  ${ms(r.p99)}  ${ms(r.worst)}  ${String(r.slow).padStart(5)}  ${String(r.blankFrames).padStart(5)}  ${ms(r.longestBlank)} ms`,
    );
  }
  const frames = results.reduce((n, r) => n + r.frames, 0);
  const slow = results.reduce((n, r) => n + r.slow, 0);
  const blank = results.reduce((n, r) => n + r.blankFrames, 0);
  lines.push(
    "",
    `Slow frames: ${slow} of ${frames} (${((100 * slow) / Math.max(1, frames)).toFixed(1)}%).  Frames with a blank row: ${blank}.`,
    "Times are ms per frame. 'slow' = more than 1.5 display intervals.",
    `Ends reachable: top ${firstSeen === 0 ? "yes" : "NO"}, bottom ${lastSeen === list.rowCount() - 1 ? "yes" : `NO (stopped at row ${lastSeen + 1} of ${list.rowCount()})`}.`,
  );
  return lines.join("\n");
}
