import type { DraftCard } from "@/api/requests";
import { kindText, taskPath } from "@/features/board/card";

/** The kind chip of a draft card, in the words the board uses; null for the default kind, which the board leaves unmarked too. */
export function cardKindLabel(card: Pick<DraftCard, "kind">): string | null {
  return kindText({ kind: card.kind ?? "proposal" });
}

export interface NeedLink {
  id: string;
  href: string;
}

/** The existing tasks a card needs, in the drafter's order, each with the link to its task page; empty for a card with none. */
export function cardNeeds(
  card: Pick<DraftCard, "needs">,
  slug: string,
): NeedLink[] {
  return (card.needs ?? []).map((id) => ({ id, href: taskPath(slug, id) }));
}
