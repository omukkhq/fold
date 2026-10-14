import type { BoardCard, BoardLabel } from "@/api/board";
import type { BoardSearch } from "./search";

/** The agent of a worker label: `frontend/ui` belongs to `frontend`, and a bare `frontend` is its own main worker. */
export function agentOf(worker: string): string {
  const slash = worker.indexOf("/");
  return slash < 0 ? worker : worker.slice(0, slash);
}

function matchesText(card: BoardCard, q: string): boolean {
  const needle = q.toLowerCase();
  return (
    card.task.toLowerCase() === needle ||
    card.title.toLowerCase().includes(needle)
  );
}

/** Every filter that is set must match. */
export function applyFilters(
  cards: readonly BoardCard[],
  filters: BoardSearch,
): BoardCard[] {
  const q = filters.q?.trim();
  return cards.filter(
    (card) =>
      (filters.agent === undefined ||
        (card.worker !== null && agentOf(card.worker) === filters.agent)) &&
      (filters.label === undefined ||
        card.labels.some((label) => label.name === filters.label)) &&
      (!filters.blocked || card.blocked) &&
      (!filters.needs || card.attention.length > 0) &&
      (!q || matchesText(card, q)),
  );
}

/** The labels the board's cards carry, each once, by name. */
export function labelOptions(cards: readonly BoardCard[]): BoardLabel[] {
  const byName = new Map<string, BoardLabel>();
  for (const card of cards) {
    for (const label of card.labels) {
      if (!byName.has(label.name)) {
        byName.set(label.name, label);
      }
    }
  }
  return [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
}

export function needsYouCount(cards: readonly BoardCard[]): number {
  return cards.filter((card) => card.attention.length > 0).length;
}
