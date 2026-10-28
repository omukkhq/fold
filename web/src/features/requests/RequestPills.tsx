import {
  Bug,
  CircleDashed,
  CircleHelp,
  Sparkles,
  type LucideIcon,
} from "lucide-react";
import type { RequestSummary } from "@/api/requests";
import { Chip } from "@/components/badge/Chip";
import { KIND_LABELS, stateChip, type KindKey } from "./present";

export const KIND_ICONS: Record<KindKey, LucideIcon> = {
  bug: Bug,
  feature: Sparkles,
  question: CircleHelp,
  unsorted: CircleDashed,
};

export function StatePill({
  request,
  human,
}: {
  request: RequestSummary;
  human: string | null;
}) {
  const chip = stateChip(request, human);
  return (
    <span data-state={request.state} className="inline-flex">
      <Chip tone={chip.tone}>{chip.label}</Chip>
    </span>
  );
}

/** The kind as its icon and word in the text colour: a kind is a fact, not a state that waits for anyone. */
export function KindLabel({ kind }: { kind: KindKey }) {
  const Icon = KIND_ICONS[kind];
  return (
    <span
      data-kind={kind}
      className="inline-flex items-center gap-6 font-medium whitespace-nowrap text-text"
    >
      <Icon aria-hidden="true" size={14} className="shrink-0" />
      {KIND_LABELS[kind]}
    </span>
  );
}
