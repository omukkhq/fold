export type Rgba = readonly [number, number, number, number];

export function parseHex(hex: string): Rgba {
  const m = /^#([0-9a-f]{6})([0-9a-f]{2})?$/i.exec(hex);
  if (!m?.[1]) {
    throw new Error(`not a hex colour: ${hex}`);
  }
  const n = parseInt(m[1], 16);
  const alpha = m[2] ? parseInt(m[2], 16) / 255 : 1;
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255, alpha];
}

export function composite(top: Rgba, bottom: Rgba): Rgba {
  const a = top[3];
  const mix = (i: 0 | 1 | 2) => top[i] * a + bottom[i] * (1 - a);
  return [mix(0), mix(1), mix(2), 1];
}

function luminance(c: Rgba): number {
  const lin = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
}

export function contrastRatio(a: Rgba, b: Rgba): number {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}
