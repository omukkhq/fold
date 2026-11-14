import type { BoardCard, BoardLabel } from "@/api/board";
import { ownsCard, type Owners } from "./owners";
import type { BoardSearch } from "./search";

function matchesText(card: BoardCard, q: string): boolean {
  const needle = q.toLowerCase();
  return (
    card.task.toLowerCase() === needle ||
    card.title.toLowerCase().includes(needle)
  );
}

/** Every filter that is set must match; inside the Labels and Owner filters one ticked entry is enough. `owners` is null until the workers answer arrives. */
export function applyFilters(
  cards: readonly BoardCard[],
  filters: BoardSearch,
  owners: Owners | null,
): BoardCard[] {
  const q = filters.q?.trim();
  const labels = filters.label ?? [];
  const keys = filters.owner ?? [];
  return cards.filter(
    (card) =>
      (keys.length === 0 || keys.some((key) => ownsCard(key, card, owners))) &&
      (labels.length === 0 ||
        card.labels.some((label) => labels.includes(label.name))) &&
      (!filters.blocked || card.blocked) &&
      (!filters.needs || card.attention.length > 0) &&
      (!q || matchesText(card, q)),
  );
}

/** The labels the board's cards carry, each once, by name, with a ticked label no card carries any more kept so it can be unticked. */
export function labelOptions(
  cards: readonly BoardCard[],
  ticked: readonly string[] = [],
): BoardLabel[] {
  const byName = new Map<string, BoardLabel>();
  for (const card of cards) {
    for (const label of card.labels) {
      if (!byName.has(label.name)) {
        byName.set(label.name, label);
      }
    }
  }
  const options = [...byName.values()].sort((a, b) =>
    a.name.localeCompare(b.name),
  );
  const gone = ticked
    .filter((name) => !byName.has(name))
    .map((name) => ({ name, color: "stone" }));
  return [...options, ...gone];
}

/** A filter's trigger names its one ticked entry, or counts several. */
export function choiceTitle(
  title: { none: string; one: string; many: string },
  chosen: readonly string[],
): string {
  const [first] = chosen;
  if (first === undefined) return title.none;
  return chosen.length === 1
    ? `${title.one}: ${first}`
    : `${title.many}: ${String(chosen.length)}`;
}

export function needsYouCount(cards: readonly BoardCard[]): number {
  return cards.filter((card) => card.attention.length > 0).length;
}
