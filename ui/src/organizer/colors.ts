// Text that stays readable on a colour somebody else chose.
//
// A calendar's colour comes from its server and can be anything: neon green,
// pale pink, navy. Wherever that colour is used as a background, the text on
// it is black or white, whichever contrasts more (WCAG relative luminance).

let canvas: CanvasRenderingContext2D | null | undefined;
const known = new Map<string, string>();

/** The colour as [r, g, b], whatever CSS spelling it came in. */
function rgb(color: string): [number, number, number] | undefined {
  if (canvas === undefined) canvas = document.createElement("canvas").getContext("2d");
  if (!canvas) return undefined;
  // An unknown colour leaves the previous value in place: try two and
  // accept only an answer that does not depend on what was there before.
  canvas.fillStyle = "#000000";
  canvas.fillStyle = color;
  const first = canvas.fillStyle;
  canvas.fillStyle = "#ffffff";
  canvas.fillStyle = color;
  if (canvas.fillStyle !== first) return undefined;
  const hex = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(first);
  if (hex) return [parseInt(hex[1]!, 16), parseInt(hex[2]!, 16), parseInt(hex[3]!, 16)];
  const parts = /^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)/.exec(first);
  return parts ? [Number(parts[1]), Number(parts[2]), Number(parts[3])] : undefined;
}

export function luminance([r, g, b]: [number, number, number]): number {
  const channel = (v: number) => {
    const c = v / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** Black or white: the one that reads better on `color`. */
export function inkOn(color: string): string {
  let ink = known.get(color);
  if (!ink) {
    const parts = rgb(color);
    const l = parts ? luminance(parts) : 0;
    // Contrast with black is (l + 0.05) / 0.05; with white, 1.05 / (l + 0.05).
    ink = (l + 0.05) / 0.05 >= 1.05 / (l + 0.05) ? "#000000" : "#ffffff";
    known.set(color, ink);
  }
  return ink;
}
