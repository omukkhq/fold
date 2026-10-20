import { MAIN_TAG } from "@/api/agents";
import type { FeedbackRecipient } from "@/api/tasks";
import { recipientValue } from "./text";

export type RecipientOption = {
  value: string;
  label: string;
};

/** The reader options of the feedback box, each reader once: the claim holder first, then the other agents, then every tag but the main tag, which reaches only the main agent and that agent is already listed by name. */
export function recipientOptions(
  holder: string | null,
  targets: { agents: string[]; tags: string[] } | undefined,
): RecipientOption[] {
  const option = (to: FeedbackRecipient, label: string): RecipientOption => ({
    value: recipientValue(to),
    label,
  });
  const options: RecipientOption[] = [];
  if (holder !== null) {
    options.push(option({ kind: "holder" }, `${holder} (claim holder)`));
  }
  for (const agent of targets?.agents ?? []) {
    if (agent !== holder) {
      options.push(option({ kind: "agent", name: agent }, agent));
    }
  }
  for (const tag of targets?.tags ?? []) {
    if (tag !== MAIN_TAG) {
      options.push(option({ kind: "tag", name: tag }, `tag ${tag}`));
    }
  }
  return options;
}
