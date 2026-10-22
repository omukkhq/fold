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
    className: "text-text-muted",
  },
  passed: {
    icon: CircleCheck,
    label: "passed",
    className: "text-primary-text",
  },
  failed: { icon: CircleX, label: "failed", className: "text-danger" },
  stopped: {
    icon: CircleMinus,
    label: "stopped",
    className: "text-text-muted",
  },
};

export const OUTCOME_ICONS: Record<
  CheckOutcome,
  { icon: LucideIcon; className: string }
> = {
  passed: { icon: CircleCheck, className: "text-primary-text" },
  failed: { icon: CircleX, className: "text-danger" },
  timed_out: { icon: Clock, className: "text-danger" },
  skipped: { icon: CircleDashed, className: "text-text-muted" },
  not_run: { icon: CircleMinus, className: "text-text-muted" },
};
