import type { BoardCard } from "@/api/board";
import type { WorkerList } from "@/api/workers";

/** The owner key of the human tasks. */
export const HUMAN_OWNER = "human";
/** The owner key of the tasks of the built-in area Main, which the main agent builds itself. */
export const MAIN_OWNER = "main";
const AREA_PREFIX = "area:";

/** An area lead's key names its area, not the lead: a lead's name changes with a restart under another name, and an area may be named `human`. */
export function areaOwner(area: string): string {
  return `${AREA_PREFIX}${area}`;
}

export interface OwnerOption {
  key: string;
  label: string;
  /** The area of an area lead's entry. */
  hint?: string;
}

/** What tells a card's owner, read from the workers answer. */
export interface Owners {
  mainLabel: string;
  /** Each area's task label, by area name. */
  areaLabels: ReadonlyMap<string, string>;
  options: OwnerOption[];
}

/**
 * Human, Main agent and one entry per area, in the answer's name order. There is no entry for the review lead: the Review column shows the reviews.
 * An area whose lead has not started yet is named by its area, so its tasks stay reachable.
 */
export function ownersOf(list: WorkerList): Owners {
  const areaLabels = new Map<string, string>();
  const options: OwnerOption[] = [
    { key: HUMAN_OWNER, label: "Human" },
    { key: MAIN_OWNER, label: "Main agent" },
  ];
  for (const area of list.areas) {
    areaLabels.set(area.name, area.label);
    options.push(
      area.lead === null
        ? { key: areaOwner(area.name), label: `${area.name} lead` }
        : { key: areaOwner(area.name), label: area.lead, hint: area.name },
    );
  }
  return { mainLabel: list.main_label, areaLabels, options };
}

/** True when an owner key needs the workers answer to match a card; only Human is told by the card alone. */
export function needsOwners(keys: readonly string[] | undefined): boolean {
  return (keys ?? []).some((key) => key !== HUMAN_OWNER);
}

/** Human owns the human tasks; Main agent the tasks with the main label; an area lead every task with its area's label, in every column. An unknown key owns nothing. */
export function ownsCard(
  key: string,
  card: BoardCard,
  owners: Owners | null,
): boolean {
  if (key === HUMAN_OWNER) return card.kind === "human";
  if (owners === null) return false;
  const label =
    key === MAIN_OWNER
      ? owners.mainLabel
      : key.startsWith(AREA_PREFIX)
        ? owners.areaLabels.get(key.slice(AREA_PREFIX.length))
        : undefined;
  return label !== undefined && card.labels.some((l) => l.name === label);
}

/** The menu's entries, with a ticked key the answer does not know kept at the end so it can be unticked. */
export function ownerChoices(
  owners: Owners | null,
  ticked: readonly string[] | undefined,
): OwnerOption[] {
  const options = owners?.options ?? [
    { key: HUMAN_OWNER, label: "Human" },
    { key: MAIN_OWNER, label: "Main agent" },
  ];
  const known = new Set(options.map((option) => option.key));
  const stale = (ticked ?? [])
    .filter((key) => !known.has(key))
    .map((key) => ({
      key,
      label: key.startsWith(AREA_PREFIX)
        ? `${key.slice(AREA_PREFIX.length)} lead`
        : key,
    }));
  return [...options, ...stale];
}
