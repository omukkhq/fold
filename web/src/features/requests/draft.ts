import type {
  DraftCard,
  DraftEdge,
  NewDraft,
  RequestDraft,
} from "@/api/requests";
import {
  MAX_CARDS,
  MAX_TEXT_CHARS,
  MAX_TITLE_CHARS,
  charCount,
  draftReasonText,
} from "./errors";

/** The requester's working copy of a draft in edit mode. */
export interface DraftForm {
  cards: DraftCard[];
  edges: DraftEdge[];
}

export function formFrom(draft: RequestDraft): DraftForm {
  return {
    cards: draft.cards.map((c) => ({ ...c })),
    edges: draft.edges.map((e) => ({ ...e })),
  };
}

/** The next free key in the drafter's `c<n>` style, so a new card never takes the key of one that was removed in the same edit. */
export function nextKey(cards: readonly DraftCard[]): string {
  const taken = new Set(cards.map((c) => c.key));
  let n = cards.length + 1;
  while (taken.has(`c${String(n)}`)) n += 1;
  return `c${String(n)}`;
}

export function addCard(form: DraftForm): DraftForm {
  if (form.cards.length >= MAX_CARDS) return form;
  return {
    ...form,
    cards: [
      ...form.cards,
      { key: nextKey(form.cards), title: "", description: "" },
    ],
  };
}

/** Removing a card removes the edges that name it, so no edge is left pointing at nothing. */
export function removeCard(form: DraftForm, key: string): DraftForm {
  return {
    cards: form.cards.filter((c) => c.key !== key),
    edges: form.edges.filter(
      (e) => e.dependent !== key && e.prerequisite !== key,
    ),
  };
}

export function updateCard(
  form: DraftForm,
  key: string,
  change: Partial<Pick<DraftCard, "title" | "description">>,
): DraftForm {
  return {
    ...form,
    cards: form.cards.map((c) => (c.key === key ? { ...c, ...change } : c)),
  };
}

/** Card order is the order an approval creates the tasks in. */
export function moveCard(form: DraftForm, key: string, by: -1 | 1): DraftForm {
  const from = form.cards.findIndex((c) => c.key === key);
  const to = from + by;
  if (from < 0 || to < 0 || to >= form.cards.length) return form;
  const cards = [...form.cards];
  const [card] = cards.splice(from, 1);
  if (card) cards.splice(to, 0, card);
  return { ...form, cards };
}

export function addEdge(form: DraftForm, edge: DraftEdge): DraftForm {
  return { ...form, edges: [...form.edges, edge] };
}

export function removeEdge(form: DraftForm, index: number): DraftForm {
  return { ...form, edges: form.edges.filter((_, i) => i !== index) };
}

/** The first edge the form does not hold yet between two different cards, for the Add edge row; null when every pair is taken. */
export function freeEdge(form: DraftForm): DraftEdge | null {
  for (const dependent of form.cards) {
    for (const prerequisite of form.cards) {
      if (dependent.key === prerequisite.key) continue;
      const taken = form.edges.some(
        (e) =>
          e.dependent === dependent.key && e.prerequisite === prerequisite.key,
      );
      if (!taken)
        return { dependent: dependent.key, prerequisite: prerequisite.key };
    }
  }
  return null;
}

export interface Refusal {
  reason: string;
  details: Record<string, unknown>;
}

/** The keys from `start` back to itself when the edge `start` needs `next` closes a cycle over `edges`, which hold no cycle themselves, so the walk ends. */
function cyclePath(
  edges: readonly DraftEdge[],
  start: string,
  next: string,
): string[] | null {
  const needs = (key: string) =>
    edges.filter((e) => e.dependent === key).map((e) => e.prerequisite);
  const walk = (key: string, path: string[]): string[] | null => {
    if (key === start) return [...path, key];
    for (const n of needs(key)) {
      const found = walk(n, [...path, key]);
      if (found) return found;
    }
    return null;
  };
  const found = walk(next, []);
  return found ? [start, ...found] : null;
}

const KEY = /^[A-Za-z0-9_-]{1,32}$/;

/** The first refusal the daemon would give, in its order; null when the draft can be saved. */
export function checkDraft(form: DraftForm): Refusal | null {
  const { cards, edges } = form;
  if (cards.length === 0) return { reason: "empty", details: {} };
  if (cards.length > MAX_CARDS) {
    return { reason: "too_many_cards", details: { cards: cards.length } };
  }
  const keys = new Set<string>();
  for (const [index, card] of cards.entries()) {
    const key = card.key;
    if (!KEY.test(key))
      return { reason: "invalid_key", details: { position: index + 1 } };
    if (keys.has(key)) return { reason: "duplicate_key", details: { key } };
    keys.add(key);
    const title = card.title.trim();
    if (title === "") return { reason: "empty_title", details: { key } };
    if (/[\n\r]/.test(title))
      return { reason: "title_not_one_line", details: { key } };
    if (charCount(title) > MAX_TITLE_CHARS)
      return { reason: "title_too_long", details: { key } };
    if (card.description.trim() === "")
      return { reason: "empty_description", details: { key } };
    if (charCount(card.description) > MAX_TEXT_CHARS) {
      return { reason: "description_too_long", details: { key } };
    }
  }
  const accepted: DraftEdge[] = [];
  for (const [index, edge] of edges.entries()) {
    if (!keys.has(edge.dependent) || !keys.has(edge.prerequisite)) {
      return { reason: "unknown_key", details: { position: index + 1 } };
    }
    if (
      accepted.some(
        (e) =>
          e.dependent === edge.dependent &&
          e.prerequisite === edge.prerequisite,
      )
    ) {
      return { reason: "duplicate_edge", details: { edge: { ...edge } } };
    }
    if (edge.dependent === edge.prerequisite) {
      return { reason: "self_edge", details: { key: edge.dependent } };
    }
    const path = cyclePath(accepted, edge.dependent, edge.prerequisite);
    if (path) return { reason: "cycle", details: { path } };
    accepted.push(edge);
  }
  return null;
}

export function refusalText(refusal: Refusal): string {
  return draftReasonText(refusal.reason, refusal.details);
}

/** The body of the edit route: titles trimmed as the daemon stores them, and the note left out when blank. */
export function draftBody(form: DraftForm, note: string): NewDraft {
  const trimmed = note.trim();
  return {
    cards: form.cards.map((c) => ({ ...c, title: c.title.trim() })),
    edges: form.edges,
    ...(trimmed === "" ? {} : { note: trimmed }),
  };
}
