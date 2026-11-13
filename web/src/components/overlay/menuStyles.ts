/** Radix gives every dropdown menu the room left between its trigger and the window edge, so a long menu stops there and scrolls inside; a focused item scrolls into view. */
export const menuContent =
  "z-pop max-h-(--radix-dropdown-menu-content-available-height) overflow-y-auto overscroll-contain rounded-m bg-surface p-4 shadow-pop";

export const menuItem =
  "flex cursor-default items-center gap-8 rounded-s px-10 py-7 text-14 text-text outline-none data-[highlighted]:bg-sunken data-[state=checked]:font-semibold";
