export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
export type ButtonSize = "md" | "sm";

// A stroke sits inside the box, so it is an inset ring: a CSS border would add 2px to the size.
// `leading-snug` follows the type size in each string, because tailwind-merge drops a leading that comes before a size.
const BASE =
  "inline-flex items-center justify-center gap-6 rounded-m font-medium whitespace-nowrap disabled:cursor-not-allowed disabled:opacity-60";

const VARIANTS: Record<ButtonVariant, string> = {
  primary: "bg-ink font-semibold text-on-ink",
  secondary:
    "bg-surface text-text inset-ring inset-ring-border hover:bg-sunken",
  ghost: "text-text-muted hover:bg-sunken",
  danger:
    "bg-danger-soft font-semibold text-danger inset-ring inset-ring-danger",
};

/** A ghost button pads 7 by 10 and the others 7 by 14; the small size is the banner's Retry, 3 by 8. */
function sizeClasses(variant: ButtonVariant, size: ButtonSize): string {
  if (size === "sm") {
    return "px-8 py-3 text-code leading-snug";
  }
  return `${variant === "ghost" ? "px-10" : "px-14"} py-7 text-body leading-snug`;
}

const ICON_SIZES: Record<ButtonSize, string> = {
  md: "size-32 text-body",
  sm: "size-24 text-code",
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

/** A secondary button draws its icon muted beside a full-strength label; the other variants share one colour. */
export function buttonIconClass(variant: ButtonVariant): string {
  return variant === "secondary"
    ? "inline-flex text-text-muted"
    : "inline-flex";
}

export function iconPixels(size: ButtonSize): number {
  return size === "sm" ? 13 : 15;
}

/** A pending control is disabled, so it cannot be activated twice. */
export function isInert(
  disabled: boolean | undefined,
  pending: boolean | undefined,
): boolean {
  return Boolean(disabled) || Boolean(pending);
}
