/**
 * How much of each pixel column of the timeline is kept, 0..1: a long recording puts many ranges
 * in one pixel, and drawing each at a minimum width would paint the whole bar as kept.
 * `ranges` is flattened `[start0, end0, start1, end1, ...]` in frames of `total`.
 */
export function coverage(ranges: number[], total: number, width: number): Float32Array {
  const out = new Float32Array(width);
  if (total <= 0 || width <= 0) return out;
  const scale = width / total;
  for (let i = 0; i < ranges.length; i += 2) {
    const a = ranges[i] * scale;
    const b = Math.min(width, ranges[i + 1] * scale);
    let x = Math.floor(a);
    while (x < b) {
      const end = Math.min(b, x + 1);
      out[x] += end - Math.max(a, x);
      x += 1;
    }
  }
  return out;
}
