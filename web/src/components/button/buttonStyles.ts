/** `primary` is the one main action of a screen; `quiet` sits on the fill tone; `ghost` has no fill until hovered; `danger` says its action in the bad tone. */
export type ButtonVariant = "primary" | "quiet" | "ghost" | "danger";
export type ButtonSize = "md" | "sm";

/** A control that can send a write names what it says while the write is in flight, so no write control keeps its idle label. */
export type PendingProps =
  | { pending?: undefined; pendingLabel?: undefined }
  | {
      /** The write is in flight: the control shows a spinner, says it is busy and cannot be pressed. */
      pending: boolean;
      /** The label while pending: the "-ing" form of the idle label, ending in "…". */
      pendingLabel: string;
    };

/** The look of a control that cannot be pressed, shared by the buttons and by any control drawn as a row or a card, so every disabled control reads the same. */
export const DISABLED_CLASSES =
  "disabled:cursor-not-allowed disabled:opacity-60";

// Buttons draw no outline: the fill tells a quiet button from the surface. `leading-tight` follows the type size in each string, because tailwind-merge drops a leading that comes before a size.
const BASE = `inline-flex items-center justify-center gap-8 rounded-s font-title font-semibold whitespace-nowrap ${DISABLED_CLASSES}`;

const VARIANTS: Record<ButtonVariant, string> = {
  primary: "bg-ink text-on-ink",
  quiet: "bg-fill text-text hover:bg-line",
  ghost: "text-text-2 hover:bg-sunken hover:text-text",
  danger: "bg-bad-soft text-bad",
};

/** A ghost button pads 7 by 10 and the others 7 by 14; the small size pads 4 by 10. */
function sizeClasses(variant: ButtonVariant, size: ButtonSize): string {
  if (size === "sm") {
    return "px-10 py-4 text-13 leading-tight";
  }
  return `${variant === "ghost" ? "px-10" : "px-14"} py-7 text-14 leading-tight`;
}

const ICON_SIZES: Record<ButtonSize, string> = {
  md: "size-32 text-14",
  sm: "size-24 text-13",
};

export function buttonClasses(
  variant: ButtonVariant,
  size: ButtonSize,
): string {
  return `${BASE} ${VARIANTS[variant]} ${sizeClasses(variant, size)}`;
}

/** An icon button is square, so it takes no text padding. */
export function iconButtonClasses(
  variant: ButtonVariant,
  size: ButtonSize,
): string {
  return `${BASE} ${VARIANTS[variant]} ${ICON_SIZES[size]}`;
}

/** A quiet or ghost button draws its icon in the secondary tone beside a full-strength label; the others share one colour. */
export function buttonIconClass(variant: ButtonVariant): string {
  return variant === "quiet" ? "inline-flex text-text-2" : "inline-flex";
}

export function iconPixels(size: ButtonSize): number {
  return size === "sm" ? 14 : 16;
}

/** A pending control is disabled, so it cannot be activated twice. */
export function isInert(
  disabled: boolean | undefined,
  pending: boolean | undefined,
): boolean {
  return Boolean(disabled) || Boolean(pending);
}

/** What a control says: its pending label while the write is in flight, its idle label otherwise. */
export function controlLabel<T>(
  idle: T,
  pending: boolean | undefined,
  pendingLabel: string | undefined,
): T | string {
  return pending && pendingLabel !== undefined ? pendingLabel : idle;
}
