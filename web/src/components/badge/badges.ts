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
  /** Padding and type size; the frames draw three badge shapes. */
  shape: string;
}

// W07's proposal pill, W02's attention mark and W00's Blocked mark. The leading follows the size, because
// tailwind-merge drops a leading that comes before a size.
const PILL = "px-8 py-2 text-meta leading-snug";
const ATTENTION = "px-7 py-2 text-meta leading-snug";
const BLOCKED = "px-8 py-3 text-caption leading-snug";

const SPECS: Record<BadgeTone, BadgeSpec> = {
  "proposal-changes": {
    icon: CircleAlert,
    text: "Changes requested",
    classes: "bg-danger-soft text-danger",
    shape: PILL,
  },
  "proposal-conflict": {
    icon: GitMerge,
    text: "Conflict",
    classes: "bg-danger-soft text-danger",
    shape: PILL,
  },
  "human-review": {
    icon: UserCheck,
    text: "Human review",
    classes: "bg-secondary-soft text-secondary-text",
    shape: PILL,
  },
  "needs-you": {
    icon: Hand,
    text: "Needs you",
    classes: "bg-ink text-on-ink",
    shape: ATTENTION,
  },
  blocked: {
    icon: OctagonAlert,
    text: "Blocked",
    classes: "bg-danger-soft text-danger",
    shape: BLOCKED,
  },
  stalled: {
    icon: Clock,
    text: "Stalled",
    classes: "bg-secondary-soft text-secondary-text",
    shape: PILL,
  },
  stale: {
    icon: History,
    text: "Stale",
    classes: "bg-ink-soft text-text-muted",
    shape: PILL,
  },
  outdated: {
    icon: History,
    text: "Outdated",
    classes: "bg-sunken text-text-muted",
    shape: PILL,
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
