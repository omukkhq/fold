export const LABEL_NAMES = [
  "sage",
  "slate",
  "teal",
  "ink",
  "gold",
  "moss",
  "plum",
  "stone",
] as const;

export type LabelName = (typeof LABEL_NAMES)[number];

export interface LabelColors {
  name: LabelName;
  /** The class for the dot, from the `label-<name>` token. */
  dot: string;
  /** The class for the chip fill, from the `label-<name>-soft` token. */
  soft: string;
}

// Full class names, so the Tailwind scanner sees every one.
const COLORS: Record<LabelName, LabelColors> = {
  sage: { name: "sage", dot: "bg-label-sage", soft: "bg-label-sage-soft" },
  slate: { name: "slate", dot: "bg-label-slate", soft: "bg-label-slate-soft" },
  teal: { name: "teal", dot: "bg-label-teal", soft: "bg-label-teal-soft" },
  ink: { name: "ink", dot: "bg-label-ink", soft: "bg-label-ink-soft" },
  gold: { name: "gold", dot: "bg-label-gold", soft: "bg-label-gold-soft" },
  moss: { name: "moss", dot: "bg-label-moss", soft: "bg-label-moss-soft" },
  plum: { name: "plum", dot: "bg-label-plum", soft: "bg-label-plum-soft" },
  stone: { name: "stone", dot: "bg-label-stone", soft: "bg-label-stone-soft" },
};

function isLabelName(name: string): name is LabelName {
  return (LABEL_NAMES as readonly string[]).includes(name);
}

/** The colours of a label colour name; an unknown name takes the `stone` pair. */
export function labelColors(name: string): LabelColors {
  return isLabelName(name) ? COLORS[name] : COLORS.stone;
}
