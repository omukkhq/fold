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

export const OUTCOME_ICONS: Record<
  CheckOutcome,
  { icon: LucideIcon; className: string }
> = {
  passed: { icon: CircleCheck, className: "text-ok" },
  failed: { icon: CircleX, className: "text-bad" },
  timed_out: { icon: Clock, className: "text-bad" },
  skipped: { icon: CircleDashed, className: "text-text-2" },
  not_run: { icon: CircleMinus, className: "text-text-2" },
};
