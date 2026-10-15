import type { ReviewCheckout } from "../src/api/reviews";

const NOW = Date.now();

export function checkouts(slug: string): ReviewCheckout[] {
  return slug === "demo"
    ? [
        {
          workspace: "ws-12",
          path: "ws/review-p-4",
          proposal: "P-4",
          version: 2,
          worker: "review/quality",
          last_command: {
            result: "ran cargo nextest, 48 passed",
            at: NOW - 2 * 60_000,
          },
        },
      ]
    : [];
}
