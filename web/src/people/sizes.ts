import type { PersonKind } from "./names";

/** Where a person is drawn. A frame that draws an agent and a human at different sizes in one place gives that place a slot. */
export type PersonSlot =
  | "messageHead"
  | "requester"
  | "gate"
  | "waitingChip"
  | "eventActor"
  | "authorCell"
  | "waitingLine"
  | "foot";

type Size = "text-meta" | "text-code" | "text-bar" | "text-body";

/** The type step each frame draws per slot and kind: 12 meta, 12.5 code, 13 bar, 14 body. The frames draw 13.5 at the author cell, the waiting lines and the foot; no token has that size, and bar is the nearest. */
const SLOT_SIZES: Record<PersonSlot, Record<PersonKind, Size>> = {
  messageHead: { agent: "text-meta", human: "text-bar" },
  requester: { agent: "text-bar", human: "text-body" },
  gate: { agent: "text-bar", human: "text-body" },
  waitingChip: { agent: "text-meta", human: "text-bar" },
  eventActor: { agent: "text-code", human: "text-bar" },
  authorCell: { agent: "text-bar", human: "text-bar" },
  waitingLine: { agent: "text-code", human: "text-bar" },
  foot: { agent: "text-bar", human: "text-bar" },
};

export function slotSize(slot: PersonSlot, kind: PersonKind): string {
  return SLOT_SIZES[slot][kind];
}
