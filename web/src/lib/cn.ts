import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// The theme resets Tailwind's default scales, so tailwind-merge must learn the token names:
// otherwise `text-meta` (a size) and `text-muted` (a colour) look like the same group.
const twMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      "font-size": [
        {
          text: [
            "page",
            "screen",
            "wordmark",
            "section",
            "card",
            "body",
            "bar",
            "micro",
            "caption",
            "meta",
            "code",
            "label",
            "overline",
          ],
        },
      ],
      "font-weight": [{ font: ["regular", "medium", "semibold", "bold"] }],
      shadow: [{ shadow: ["card", "dialog"] }],
      rounded: [{ rounded: ["xs", "s", "m", "l"] }],
    },
  },
});

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
