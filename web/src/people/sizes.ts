import type { PersonKind } from "./names";

/** Where a person is drawn. A place that shows an agent and a human at different sizes has a slot. */
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

type Size = "text-13" | "text-14" | "text-15" | "text-16";

/** The type step per slot and kind: 13 metadata, 14 rows and controls, 15 body, 16 titles. */
const SLOT_SIZES: Record<PersonSlot, Record<PersonKind, Size>> = {
  messageHead: { agent: "text-13", human: "text-14" },
  requester: { agent: "text-14", human: "text-15" },
  gate: { agent: "text-14", human: "text-15" },
  waitingChip: { agent: "text-13", human: "text-14" },
  eventActor: { agent: "text-13", human: "text-14" },
  authorCell: { agent: "text-14", human: "text-14" },
  waitingLine: { agent: "text-13", human: "text-14" },
  foot: { agent: "text-14", human: "text-14" },
  teamRow: { agent: "text-15", human: "text-14" },
  teamNote: { agent: "text-16", human: "text-14" },
  needsYouLine: { agent: "text-15", human: "text-15" },
  panelName: { agent: "text-16", human: "text-16" },
  answeredBy: { agent: "text-15", human: "text-15" },
  batchHead: { agent: "text-16", human: "text-16" },
};

export function slotSize(slot: PersonSlot, kind: PersonKind): string {
  return SLOT_SIZES[slot][kind];
}
