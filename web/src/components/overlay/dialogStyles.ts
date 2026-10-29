/**
 * The scrim holds its dialog (Radix's scrollable overlay): its padding is the gutter round the dialog, so a
 * dialog never runs past the window, and the dialog's own body scrolls instead.
 */
export const dialogOverlay =
  "fixed inset-0 z-dialog flex items-center justify-center bg-scrim p-16 md:py-32";

/** The dialog box; its head and foot keep `shrink-0` and only `dialogBody` scrolls. */
export const dialogContent =
  "flex max-h-full min-h-0 max-w-full flex-col overflow-hidden rounded-l bg-surface shadow-dialog outline-none";

export const dialogBody = "min-h-0 overflow-y-auto";

/** A wide dialog takes the whole screen of a phone: the scrim drops its gutter and the box its corners. */
export const dialogFillOverlay = "max-md:p-0";
export const dialogFill = "max-md:h-full max-md:w-full max-md:rounded-none";
