import type { MessageView } from "@/api/messages";
import { isYou } from "@/people/names";
import { actorSegment, seg, type Segment } from "@/people/segments";
import { deliveryLines, type Delivery, type DeliveryTone } from "./delivery";

/** A message to more recipients than this folds its receipts into counts behind a Details button. */
export const UNFOLDED_RECIPIENTS = 2;

/** One part of the receipt line; its tone picks the icon and the colour. */
export interface ReceiptPart {
  tone: DeliveryTone;
  segments: Segment[];
}

/** The receipt line under a message, and the per-recipient lines Details opens when the line is folded. */
export interface Receipts {
  parts: ReceiptPart[];
  folded: boolean;
  details: Delivery[];
}

/** The problem states lead; among the rest, the furthest state comes first. */
const ORDER: readonly DeliveryTone[] = [
  "failed",
  "waiting",
  "forwarded",
  "read",
  "delivered",
];

function capitalize(text: string, first = true): string {
  return first ? `${text.charAt(0).toUpperCase()}${text.slice(1)}` : text;
}

/** `Read by you · 22:19`, `@web-worker-41 cannot start now · sent on to @web-lead`. */
function onePart(line: Delivery): ReceiptPart {
  const at = line.at === null ? "" : ` · ${line.at}`;
  if (line.lead === "") {
    return {
      tone: line.tone,
      segments: seg`${actorSegment(line.actor)}${at} ${line.note ?? ""}`,
    };
  }
  const note = line.note === null ? "" : ` · ${line.note}`;
  return {
    tone: line.tone,
    segments: seg`${capitalize(line.lead)} ${actorSegment(line.actor)}${at}${note}`,
  };
}

type Group = [Delivery, ...Delivery[]];

/** `Read by you and 2 others`, `delivered to 2`, `3 cannot start now`. */
function groupPart(
  lines: Group,
  you: string | null,
  first: boolean,
): ReceiptPart {
  const [line] = lines;
  const own = lines.find((l) => isYou(l.actor, you));
  const others = lines.length - 1;
  let who: Segment[];
  if (own && others > 0) {
    who = seg`${actorSegment(own.actor)} and ${String(others)} ${others === 1 ? "other" : "others"}`;
  } else if (lines.length === 1) {
    who = [actorSegment(line.actor)];
  } else {
    who = [String(lines.length)];
  }
  return {
    tone: line.tone,
    segments:
      line.lead === ""
        ? seg`${who} cannot start now`
        : seg`${capitalize(line.lead, first)} ${who}`,
  };
}

/** The receipts of a message: each recipient on the line while there are at most `UNFOLDED_RECIPIENTS`, otherwise one count per state with the worst state first. */
export function receipts(
  message: MessageView,
  now: number,
  you: string | null,
): Receipts {
  const details = deliveryLines(message, now);
  const ordered = [...details].sort(
    (a, b) => ORDER.indexOf(a.tone) - ORDER.indexOf(b.tone),
  );
  if (details.length <= UNFOLDED_RECIPIENTS) {
    return { parts: ordered.map(onePart), folded: false, details };
  }
  const groups = new Map<string, Group>();
  for (const line of ordered) {
    const key = `${line.tone}:${line.lead}`;
    const group = groups.get(key);
    if (group) group.push(line);
    else groups.set(key, [line]);
  }
  const parts = [...groups.values()].map((lines, index) =>
    groupPart(lines, you, index === 0),
  );
  return { parts, folded: true, details };
}

/** One line of the opened Details list: the lead word, the note a stopped worker carries after its name, and the time and note at the right. */
export interface DetailRow {
  tone: DeliveryTone;
  lead: string;
  actor: string;
  after: string | null;
  trailing: string;
}

export function detailRow(line: Delivery): DetailRow {
  const forwarded = line.lead === "";
  return {
    tone: line.tone,
    lead: forwarded ? "" : capitalize(line.lead),
    actor: line.actor,
    after: forwarded ? line.note : null,
    trailing: [line.at, forwarded ? null : line.note]
      .filter((part) => part !== null)
      .join(" · "),
  };
}
