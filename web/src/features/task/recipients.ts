import type { FeedbackRecipient, TaskReader } from "@/api/tasks";
import { recipientValue } from "./text";

export type RecipientOption = {
  value: string;
  label: string;
};

const ROLES: Record<TaskReader["role"], string> = {
  claim_holder: "claim holder",
  area_lead: "area lead",
  main_agent: "main agent",
};

/** The claim holder is addressed as the holder, so the daemon sends the feedback to the first reader, whoever holds the claim when it is sent; the lead and the main agent by name. */
export function readerRecipient(reader: TaskReader): FeedbackRecipient {
  return reader.role === "claim_holder"
    ? { kind: "holder" }
    : { kind: "agent", name: reader.name };
}

/** `web-t12 (claim holder)`. */
export function readerLabel(reader: TaskReader): string {
  return `${reader.name} (${ROLES[reader.role]})`;
}

/** The feedback box's reader options in the daemon's order, the default first; none before the readers are read. */
export function recipientOptions(
  readers: readonly TaskReader[] | undefined,
): RecipientOption[] {
  return (readers ?? []).map((reader) => ({
    value: recipientValue(readerRecipient(reader)),
    label: readerLabel(reader),
  }));
}

/** The reader the box keeps: the current choice while the daemon still offers it, else the default, else no reader (`""`). */
export function keptChoice(
  current: string,
  options: readonly RecipientOption[],
): string {
  if (options.some((option) => option.value === current)) {
    return current;
  }
  return options[0]?.value ?? "";
}
