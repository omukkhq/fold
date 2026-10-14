import { z } from "zod";

const text = z.string().trim().min(1).max(200).optional().catch(undefined);
const flag = z.literal(true).optional().catch(undefined);

/** The board filters as URL search params, so a filtered board can be shared by link. A malformed value is dropped, never an error. */
export const boardSearchSchema = z.object({
  agent: text,
  label: text,
  blocked: flag,
  needs: flag,
  q: text,
});

export type BoardSearch = z.infer<typeof boardSearchSchema>;

/** The search with every unset filter removed, so an unfiltered board has a clean URL. */
export function cleanSearch(search: BoardSearch): BoardSearch {
  const clean: BoardSearch = {};
  if (search.agent) clean.agent = search.agent;
  if (search.label) clean.label = search.label;
  if (search.blocked) clean.blocked = true;
  if (search.needs) clean.needs = true;
  const q = search.q?.trim();
  if (q) clean.q = q;
  return clean;
}

/** Choosing the value already chosen clears it. */
export function toggleValue(
  current: string | undefined,
  chosen: string,
): string | undefined {
  return current === chosen ? undefined : chosen;
}

export function isFiltered(search: BoardSearch): boolean {
  return Object.keys(cleanSearch(search)).length > 0;
}
