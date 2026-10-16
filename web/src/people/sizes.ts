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
  | "foot"
  | "teamRow"
  | "teamNote"
  | "needsYouLine"
  | "panelName"
  | "answeredBy"
  | "batchHead";

type Size =
  | "text-meta"
  | "text-code"
  | "text-bar"
  | "text-body"
  | "text-card"
  | "text-section";

/** The type step each frame draws per slot and kind: 12 meta, 12.5 code, 13 bar, 14 body, 15 card, 16 section. The frames draw 13.5 at the author cell, the waiting lines, the foot and the team's group notes; no token has that size, and bar is the nearest. */
const SLOT_SIZES: Record<PersonSlot, Record<PersonKind, Size>> = {
  messageHead: { agent: "text-meta", human: "text-bar" },
  requester: { agent: "text-bar", human: "text-body" },
  gate: { agent: "text-bar", human: "text-body" },
  waitingChip: { agent: "text-meta", human: "text-bar" },
  eventActor: { agent: "text-code", human: "text-bar" },
  authorCell: { agent: "text-bar", human: "text-bar" },
  waitingLine: { agent: "text-code", human: "text-bar" },
  foot: { agent: "text-bar", human: "text-bar" },
  teamRow: { agent: "text-body", human: "text-bar" },
  teamNote: { agent: "text-card", human: "text-bar" },
  needsYouLine: { agent: "text-body", human: "text-body" },
  panelName: { agent: "text-section", human: "text-section" },
  answeredBy: { agent: "text-body", human: "text-body" },
  batchHead: { agent: "text-card", human: "text-card" },
};

export function slotSize(slot: PersonSlot, kind: PersonKind): string {
  return SLOT_SIZES[slot][kind];
}
