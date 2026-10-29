import {
  CircleCheck,
  CircleDashed,
  CircleMinus,
  CircleX,
  Clock,
  LoaderCircle,
  type LucideIcon,
} from "lucide-react";
import type { CheckOutcome, CheckRunState } from "@/api/proposals";

export const RUN_ICONS: Record<
  CheckRunState,
  { icon: LucideIcon; label: string; className: string }
> = {
  running: {
    icon: LoaderCircle,
    label: "running",
    className: "text-text-2",
  },
  passed: {
    icon: CircleCheck,
    label: "passed",
    className: "text-ok",
  },
  failed: { icon: CircleX, label: "failed", className: "text-bad" },
  stopped: {
    icon: CircleMinus,
    label: "stopped",
    className: "text-text-2",
  },
};

/** A step's icon and tone; `selected` is the row's fill while it is the open step, green only for a step that passed, so a skipped or not-run step never reads as a pass. */
export const OUTCOME_ICONS: Record<
  CheckOutcome,
  { icon: LucideIcon; className: string; selected: string }
> = {
  passed: {
    icon: CircleCheck,
    className: "text-ok",
    selected: "bg-ok-soft hover:bg-ok-soft",
  },
  failed: {
    icon: CircleX,
    className: "text-bad",
    selected: "bg-bad-soft hover:bg-bad-soft",
  },
  timed_out: {
    icon: Clock,
    className: "text-bad",
    selected: "bg-bad-soft hover:bg-bad-soft",
  },
  skipped: {
    icon: CircleDashed,
    className: "text-text-2",
    selected: "bg-accent-soft hover:bg-accent-soft",
  },
  not_run: {
    icon: CircleMinus,
    className: "text-text-2",
    selected: "bg-accent-soft hover:bg-accent-soft",
  },
};
