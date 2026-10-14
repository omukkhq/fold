const MAX_ID = 4_294_967_295;
const SLUG = /^[a-z0-9][a-z0-9-]{0,63}$/;

/** Same shape as the daemon's public ids: decimal digits, no zero, no leading zero, fits in u32. */
function parsePrefixed(prefix: string, raw: string): number | null {
  if (!raw.startsWith(`${prefix}-`)) {
    return null;
  }
  const digits = raw.slice(prefix.length + 1);
  if (!/^[1-9][0-9]{0,9}$/.test(digits)) {
    return null;
  }
  const value = Number(digits);
  return value <= MAX_ID ? value : null;
}

export function parseTaskParam(raw: string): number | null {
  return parsePrefixed("T", raw);
}

export function parseProposalParam(raw: string): number | null {
  return parsePrefixed("P", raw);
}

export function parseSlug(raw: string): string | null {
  return SLUG.test(raw) ? raw : null;
}
