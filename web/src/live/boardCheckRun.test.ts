import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { afterEach, describe, expect, it } from "vitest";
import type { Board, CardCheckRun } from "@/api/board";
import { keys } from "@/api/keys";
import { currentCheckRun } from "@/features/board/checkRun";
import { cardChip } from "@/features/board/chip";
import { boardCard } from "@/test/payloads";
import { applyEvent } from "./apply";
import { isEventEnvelope, type EventEnvelope } from "./envelope";

const slug = "demo";

function event(
  type: "checks.started" | "checks.finished",
  run: CardCheckRun,
): EventEnvelope {
  const frame: unknown = {
    kind: "event",
    seq: 11,
    type,
    actor: "fold",
    task: "T-3",
    proposal: "P-3",
    workspace: null,
    revision: null,
    data: { run: run.run, stage: run.stage, purpose: run.purpose },
    created_at: 1_760_000_000_000,
  };
  if (!isEventEnvelope(frame)) throw new Error("not a frame");
  return frame;
}

let client: QueryClient;
let stop: (() => void) | null = null;

afterEach(() => {
  stop?.();
  stop = null;
  client.clear();
});

describe("the board card and the check run events", () => {
  it.each([
    {
      run: {
        run: "cr-4",
        stage: "integration",
        purpose: "integrate",
        proposal: "P-3",
        started_at: 1_760_000_000_000,
      } satisfies CardCheckRun,
      text: "Integration checks running",
    },
    {
      run: {
        run: "cr-5",
        stage: "proposal",
        purpose: "update_proposal",
        proposal: "P-3",
        started_at: 1_760_000_000_000,
      } satisfies CardCheckRun,
      text: "Checks running",
    },
  ])(
    "a start event adds $text to the card and an end event removes it",
    async ({ run, text }) => {
      client = new QueryClient();
      // The daemon's answer to the next board read; the events only tell the board to read it again.
      let served: CardCheckRun | null = null;
      const observer = new QueryObserver(client, {
        queryKey: keys.board(slug),
        queryFn: (): Promise<Board> =>
          Promise.resolve({
            columns: [
              {
                status: "review",
                total: 1,
                cards: [
                  {
                    ...boardCard("T-3", { status: "review" }),
                    check_run: served,
                  },
                ],
              },
            ],
          }),
        staleTime: Infinity,
      });
      stop = observer.subscribe(() => undefined);
      const shown = (): string | null | undefined => {
        const card = client.getQueryData<Board>(keys.board(slug))?.columns[0]
          ?.cards[0];
        if (card === undefined) return undefined;
        const current = currentCheckRun(card.check_run, undefined);
        return current === null
          ? null
          : (cardChip(card, current)?.text ?? null);
      };
      await expect.poll(shown).toBeNull();

      served = run;
      applyEvent(client, slug, event("checks.started", run));
      await expect.poll(shown).toBe(text);

      served = null;
      applyEvent(client, slug, event("checks.finished", run));
      await expect.poll(shown).toBeNull();
    },
  );
});
