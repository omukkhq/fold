import {
  CircleAlert,
  Clock,
  GitMerge,
  Hand,
  History,
  OctagonAlert,
  UserCheck,
  type LucideIcon,
} from "lucide-react";

export const BADGE_TONES = [
  "proposal-changes",
  "proposal-conflict",
  "human-review",
  "needs-you",
  "blocked",
  "stalled",
  "stale",
  "outdated",
] as const;

export type BadgeTone = (typeof BADGE_TONES)[number];

export interface BadgeSpec {
  icon: LucideIcon;
  /** The default text; a badge always shows text, never colour alone. */
  text: string;
  /** Classes for the fill and the text and icon colour. */
  classes: string;
  /** Padding and type size, the same for every tone. */
  shape: string;
}

/** Every badge is one chip shape: 13px on a pill, no outline. */
const CHIP = "px-9 py-2 text-13 leading-tight";

const SPECS: Record<BadgeTone, BadgeSpec> = {
  "proposal-changes": {
    icon: CircleAlert,
    text: "Changes requested",
    classes: "bg-bad-soft text-bad",
    shape: CHIP,
  },
  "proposal-conflict": {
    icon: GitMerge,
    text: "Conflict",
    classes: "bg-bad-soft text-bad",
    shape: CHIP,
  },
  "human-review": {
    icon: UserCheck,
    text: "Human review",
    classes: "bg-warn-soft text-warn",
    shape: CHIP,
  },
  "needs-you": {
    icon: Hand,
    text: "Needs you",
    classes: "bg-warn-soft text-warn",
    shape: CHIP,
  },
  blocked: {
    icon: OctagonAlert,
    text: "Blocked",
    classes: "bg-bad-soft text-bad",
    shape: CHIP,
  },
  stalled: {
    icon: Clock,
    text: "Stalled",
    classes: "bg-warn-soft text-warn",
    shape: CHIP,
  },
  stale: {
    icon: History,
    text: "Stale",
    classes: "bg-fill text-text",
    shape: CHIP,
  },
  outdated: {
    icon: History,
    text: "Outdated",
    classes: "bg-fill text-text-2",
    shape: CHIP,
  },
};

export function badgeSpec(tone: BadgeTone): BadgeSpec {
  return SPECS[tone];
}

/** The text a badge shows: the caller's text when it has any, else the tone's own. */
export function badgeText(tone: BadgeTone, text?: string): string {
  const custom = text?.trim();
  return custom ? custom : SPECS[tone].text;
}
