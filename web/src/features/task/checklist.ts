import type { Checklist, ChecklistItem, StepItem } from "@/api/tasks";

export interface Progress {
  done: number;
  total: number;
  /** 0 to 100, rounded down, so a bar never shows full before the last item. */
  percent: number;
  /** `4/5`. */
  text: string;
}

/** Plain steps and linked items count together, as the board card counts them. */
export function progress(checklist: Pick<Checklist, "items">): Progress {
  const total = checklist.items.length;
  const done = checklist.items.filter((item) => item.done).length;
  return {
    done,
    total,
    percent: total === 0 ? 0 : Math.floor((done * 100) / total),
    text: `${String(done)}/${String(total)}`,
  };
}

/** Only a plain step takes a tick by hand; fold ticks a linked item when its child reaches Done. */
export function isTickable(item: ChecklistItem): item is StepItem {
  return item.kind === "step";
}

export interface MoveTargets {
  /** The position to move to, or null at the top. */
  up: number | null;
  /** The position to move to, or null at the bottom. */
  down: number | null;
}

export function moveTargets(index: number, count: number): MoveTargets {
  return {
    up: index > 0 ? index - 1 : null,
    down: index < count - 1 ? index + 1 : null,
  };
}

/** What a human reads before adding a linked item: the link is also a dependency edge. */
export function linkAddText(parent: string): string {
  return `Linking a task also adds the dependency: ${parent} will need it, and fold ticks the item when it is Done.`;
}

/** What a human reads before removing a linked item: the edge goes with it. */
export function linkRemoveText(parent: string, child: string): string {
  return `Removing the link also removes the dependency: ${parent} will no longer need ${child}.`;
}

/** The linked item's state text, `Done · ticked by fold` once the child is Done. */
export function linkedStateText(
  item: Extract<ChecklistItem, { kind: "linked" }>,
  statusText: string,
): string {
  return item.done ? `${statusText} · ticked by fold` : statusText;
}
