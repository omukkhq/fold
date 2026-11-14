import { z } from "zod";

/**
 * The router reads a bare value as JSON, so `?label=2025` comes as a number and `?label=true` as a boolean; each is read back as the text of the link.
 * A number whose JSON text differs from the link's, such as `1e3` or `1.0`, comes back in its JSON form (`1000`, `1`), and `null` is dropped: the router keeps no trace of the original.
 */
const name = z
  .union([z.string(), z.number(), z.boolean()])
  .transform(String)
  .pipe(z.string().trim().min(1).max(200));
const text = name.optional().catch(undefined);
const flag = z.literal(true).optional().catch(undefined);

/** One bare value or a JSON array, as the router gives them, is a list; a malformed entry is dropped. */
function readNames(value: unknown): string[] | undefined {
  const list: unknown[] = Array.isArray(value) ? value : [value];
  const names: string[] = [];
  for (const item of list) {
    const parsed = name.safeParse(item);
    if (parsed.success && !names.includes(parsed.data)) {
      names.push(parsed.data);
    }
  }
  return names.length > 0 ? names : undefined;
}

const names = z.unknown().transform(readNames).optional();

/**
 * The board filters as URL search params, so a filtered board can be shared by link. A malformed value is dropped, never an error.
 * `label` and `owner` hold every ticked entry; one bare value reads as a list of one, and an `agent` param is left out.
 */
export const boardSearchSchema = z.object({
  label: names,
  owner: names,
  blocked: flag,
  needs: flag,
  q: text,
});

export type BoardSearch = z.infer<typeof boardSearchSchema>;

/** The search with every unset filter removed, so an unfiltered board has a clean URL. */
export function cleanSearch(search: BoardSearch): BoardSearch {
  const clean: BoardSearch = {};
  const label = readNames(search.label);
  if (label) clean.label = label;
  const owner = readNames(search.owner);
  if (owner) clean.owner = owner;
  if (search.blocked) clean.blocked = true;
  if (search.needs) clean.needs = true;
  const q = search.q?.trim();
  if (q) clean.q = q;
  return clean;
}

/** Ticking an entry adds it to the list and ticking it again takes it out; an empty list is no filter. */
export function toggleName(
  current: readonly string[] | undefined,
  chosen: string,
): string[] | undefined {
  const list = current ?? [];
  const next = list.includes(chosen)
    ? list.filter((entry) => entry !== chosen)
    : [...list, chosen];
  return next.length > 0 ? next : undefined;
}

export function isFiltered(search: BoardSearch): boolean {
  return Object.keys(cleanSearch(search)).length > 0;
}
