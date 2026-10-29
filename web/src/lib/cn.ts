import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// The theme resets Tailwind's default scales, so tailwind-merge must learn the token names:
// otherwise `text-13` (a size) and `text-text-2` (a colour) look like the same group.
const twMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      "font-size": [
        {
          text: ["12", "13", "14", "15", "16", "18", "21", "24", "28"],
        },
      ],
      "font-weight": [{ font: ["regular", "medium", "semibold", "bold"] }],
      shadow: [{ shadow: ["pop", "dialog"] }],
      rounded: [{ rounded: ["s", "m", "l", "pill"] }],
      tracking: [{ tracking: ["wordmark"] }],
    },
  },
});

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
