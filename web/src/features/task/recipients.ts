import { MAIN_TAG } from "@/api/agents";
import type { FeedbackRecipient } from "@/api/tasks";
import { STOPPED_MARK, isStopped } from "@/features/messages/stopped";
import { recipientValue } from "./text";

export type RecipientOption = {
  value: string;
  label: string;
};

/** The reader options of the feedback box, each reader once: the claim holder first, then the other agents, then every tag but the main tag, which reaches only the main agent and that agent is already listed by name. A reader in `stopped` is marked, because the daemon starts that task worker again for the feedback. */
export function recipientOptions(
  holder: string | null,
  targets: { agents: string[]; tags: string[] } | undefined,
  stopped: ReadonlySet<string> = new Set(),
): RecipientOption[] {
  const marked = (name: string, label: string) =>
    isStopped(name, stopped) ? `${label} · ${STOPPED_MARK}` : label;
  const option = (to: FeedbackRecipient, label: string): RecipientOption => ({
    value: recipientValue(to),
    label,
  });
  const options: RecipientOption[] = [];
  if (holder !== null) {
    options.push(
      option({ kind: "holder" }, marked(holder, `${holder} (claim holder)`)),
    );
  }
  for (const agent of targets?.agents ?? []) {
    if (agent !== holder) {
      options.push(
        option({ kind: "agent", name: agent }, marked(agent, agent)),
      );
    }
  }
  for (const tag of targets?.tags ?? []) {
    if (tag !== MAIN_TAG) {
      options.push(option({ kind: "tag", name: tag }, `tag ${tag}`));
    }
  }
  return options;
}

/** The agent a feedback choice names, for the note on a stopped reader; none for a tag. */
export function feedbackReaders(
  to: FeedbackRecipient | null,
  holder: string | null,
): string[] {
  if (to === null) {
    return [];
  }
  if (to.kind === "holder") {
    return holder === null ? [] : [holder];
  }
  return to.kind === "agent" ? [to.name] : [];
}
