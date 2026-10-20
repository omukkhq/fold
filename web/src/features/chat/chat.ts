import { z } from "zod";
import type { MessageView } from "@/api/messages";
import type {
  AreaEntry,
  UpdateAreaBody,
  WorkerProcess,
  WorkerRole,
} from "@/api/workers";
import { charCount } from "@/format/text";

/** The daemon's bound on an area's instructions, which go into every start of the area's workers. */
export const MAX_INSTRUCTIONS = 8000;

/** One message of the chat as the screen draws it: the humans' messages on one side, the agent's on the other. */
export interface ChatLine {
  message: MessageView;
  fromHuman: boolean;
}

export function chatLines(messages: readonly MessageView[]): ChatLine[] {
  return messages.map((message) => ({
    message,
    fromHuman: message.sender.startsWith("human:"),
  }));
}

/** The area that `agent` leads, or null when it leads none. */
export function ledArea(
  areas: readonly AreaEntry[],
  agent: string,
): AreaEntry | null {
  return areas.find((area) => area.lead === agent) ?? null;
}

const ROLE_TEXT: Readonly<Record<WorkerRole, string>> = {
  area_lead: "area lead",
  review_lead: "review lead",
  task_worker: "task worker",
  reviewer: "reviewer",
};

/** What the agent is and how a message reaches it. An agent fold does not supervise is a Claude Code session, such as the main agent. */
export function agentFacts(
  workers: readonly WorkerProcess[],
  agent: string,
): { role: string; delivery: string } {
  const process = workers.find((worker) => worker.name === agent);
  if (!process) {
    return {
      role: "Claude Code session",
      delivery:
        "fold shows each message in its session as a notice, and it answers here.",
    };
  }
  const area = process.area === null ? "" : ` of ${process.area}`;
  const lead = process.role === "area_lead" || process.role === "review_lead";
  return {
    role: `${ROLE_TEXT[process.role]}${area}`,
    delivery: `fold gives each message to it as a turn, and it answers here.${lead ? " A request that changes the approved plan goes to the main agent." : ""}`,
  };
}

/** The change an edit of the instructions asks for: null when the text stays the same, a clear when it is empty. */
export function instructionsChange(
  current: string | null,
  next: string,
): UpdateAreaBody | null {
  const text = next.trim();
  if (text === (current ?? "")) {
    return null;
  }
  return text === "" ? { clear_instructions: true } : { instructions: text };
}

export const instructionsSchema = z.object({
  text: z
    .string()
    .refine(
      (value) => charCount(value.trim()) <= MAX_INSTRUCTIONS,
      `An area's instructions hold at most ${String(MAX_INSTRUCTIONS)} characters.`,
    ),
});

export type InstructionsValues = z.infer<typeof instructionsSchema>;
