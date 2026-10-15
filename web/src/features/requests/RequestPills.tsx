import {
  Bug,
  CircleDashed,
  CircleHelp,
  Sparkles,
  type LucideIcon,
} from "lucide-react";
import type { RequestState } from "@/api/requests";
import { cn } from "@/lib/cn";
import { KIND_SPECS, STATE_SPECS, type KindKey } from "./present";

export const KIND_ICONS: Record<KindKey, LucideIcon> = {
  bug: Bug,
  feature: Sparkles,
  question: CircleHelp,
  unsorted: CircleDashed,
};

export function StatePill({ state }: { state: RequestState }) {
  const spec = STATE_SPECS[state];
  return (
    <span
      data-state={state}
      className={cn(
        "inline-flex items-center gap-5 rounded-s px-9 py-3 text-meta leading-snug font-semibold whitespace-nowrap",
        spec.pill,
      )}
    >
      <span
        aria-hidden="true"
        className={cn("size-6 rounded-full", spec.dot)}
      />
      {spec.label}
    </span>
  );
}

export function KindBadge({ kind }: { kind: KindKey }) {
  const spec = KIND_SPECS[kind];
  const Icon = KIND_ICONS[kind];
  return (
    <span
      data-kind={kind}
      className={cn(
        "inline-flex items-center gap-5 rounded-s px-8 py-2 text-meta leading-snug font-semibold whitespace-nowrap",
        spec.classes,
      )}
    >
      <Icon aria-hidden="true" size={12} />
      {spec.label}
    </span>
  );
}
