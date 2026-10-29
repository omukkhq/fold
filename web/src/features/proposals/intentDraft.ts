import type { Intent, IntentContracts } from "@/api/proposals";
import {
  CONTRACT_KINDS,
  MAX_ENTRIES,
  MAX_ENTRY_CHARS,
  MAX_PART_CHARS,
  MAX_PART_NAME_CHARS,
  MAX_PARTS,
  MAX_SUMMARY_CHARS,
} from "./intent";

/** A part as the edit form holds it: the files one path per line. */
export interface PartDraft {
  name: string;
  does: string;
  files: string;
}

/** A section besides the parts: a contract kind, the decisions or what the change does not change. */
export type SectionKey = keyof IntentContracts | "decisions" | "not_changed";

/** The sections in the order the form lists them, with their labels. */
export const SECTIONS: readonly { key: SectionKey; label: string }[] = [
  ...CONTRACT_KINDS.map(({ kind, label }) => ({ key: kind, label })),
  { key: "decisions", label: "Decisions" },
  { key: "not_changed", label: "Not changed" },
];

/** The intent as the edit form holds it while the human writes: each list one entry per line. */
export interface IntentDraft {
  summary: string;
  parts: PartDraft[];
  sections: Record<SectionKey, string>;
}

function emptySections(): Record<SectionKey, string> {
  return {
    routes: "",
    database: "",
    rights: "",
    config: "",
    errors: "",
    events: "",
    commands: "",
    decisions: "",
    not_changed: "",
  };
}

/** The form's start: the edited version's intent, or for a version without one a single part that holds `files`, the files the version changes. */
export function draftOf(
  intent: Intent | null,
  files: readonly string[],
): IntentDraft {
  if (intent === null) {
    return {
      summary: "",
      parts: [{ name: "", does: "", files: files.join("\n") }],
      sections: emptySections(),
    };
  }
  const sections = emptySections();
  for (const { kind } of CONTRACT_KINDS) {
    sections[kind] = intent.contracts[kind].join("\n");
  }
  sections.decisions = intent.decisions.join("\n");
  sections.not_changed = intent.not_changed.join("\n");
  return {
    summary: intent.summary,
    parts: intent.parts.map((part) => ({
      name: part.name,
      does: part.does,
      files: part.files.join("\n"),
    })),
    sections,
  };
}

/** The non-blank lines of a list field, trimmed. */
export function lines(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

/** The intent the form sends: every text trimmed and every list without blank lines. */
export function intentOf(draft: IntentDraft): Intent {
  const list = (key: SectionKey) => lines(draft.sections[key]);
  return {
    summary: draft.summary.trim(),
    parts: draft.parts.map((part) => ({
      name: part.name.trim(),
      does: part.does.trim(),
      files: lines(part.files),
    })),
    contracts: {
      routes: list("routes"),
      database: list("database"),
      rights: list("rights"),
      config: list("config"),
      errors: list("errors"),
      events: list("events"),
      commands: list("commands"),
    },
    decisions: list("decisions"),
    not_changed: list("not_changed"),
  };
}

/** Whether two intents say the same, so an unchanged form keeps the edited version's intent. */
export function sameIntent(a: Intent, b: Intent): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

// The daemon counts Unicode code points, as Array.from does.
function length(text: string): number {
  return Array.from(text).length;
}

/** Why Save cannot send the intent as it stands; null when it can. The daemon checks again, and also that the parts cover exactly the files of the edit; the form checks that the edited file is in a part. */
export function draftProblem(
  draft: IntentDraft,
  editedPath: string,
): string | null {
  const intent = intentOf(draft);
  if (intent.summary === "") {
    return "Write the summary: how the change works in the code.";
  }
  if (length(intent.summary) > MAX_SUMMARY_CHARS) {
    return `The summary can hold at most ${String(MAX_SUMMARY_CHARS)} characters; it has ${String(length(intent.summary))}.`;
  }
  if (intent.parts.length === 0) return "Add a part.";
  if (intent.parts.length > MAX_PARTS) {
    return `An intent has at most ${String(MAX_PARTS)} parts.`;
  }
  const names = new Set<string>();
  const owners = new Set<string>();
  for (const [index, part] of intent.parts.entries()) {
    const which = `Part ${String(index + 1)}`;
    if (part.name === "") return `${which} needs a name.`;
    if (length(part.name) > MAX_PART_NAME_CHARS) {
      return `${which}: a name has at most ${String(MAX_PART_NAME_CHARS)} characters.`;
    }
    if (names.has(part.name)) return `Two parts are named ${part.name}.`;
    names.add(part.name);
    if (part.does === "")
      return `${which}: say what this part of the code does.`;
    if (length(part.does) > MAX_PART_CHARS) {
      return `${which}: what it does has at most ${String(MAX_PART_CHARS)} characters.`;
    }
    for (const file of part.files) {
      if (owners.has(file)) return `${file} is in more than one part.`;
      owners.add(file);
    }
  }
  if (!owners.has(editedPath)) {
    return `Put ${editedPath} in the part whose code it changes.`;
  }
  for (const { key, label } of SECTIONS) {
    const entries = lines(draft.sections[key]);
    if (entries.length > MAX_ENTRIES) {
      return `${label} has at most ${String(MAX_ENTRIES)} entries.`;
    }
    if (entries.some((entry) => length(entry) > MAX_ENTRY_CHARS)) {
      return `${label}: an entry has at most ${String(MAX_ENTRY_CHARS)} characters.`;
    }
  }
  return null;
}

/** The form with one more empty part at the end. */
export function addPart(draft: IntentDraft): IntentDraft {
  return {
    ...draft,
    parts: [...draft.parts, { name: "", does: "", files: "" }],
  };
}

/** The form without the part at `index`. */
export function removePart(draft: IntentDraft, index: number): IntentDraft {
  return { ...draft, parts: draft.parts.filter((_, at) => at !== index) };
}

/** The form with the part at `index` changed by `patch`. */
export function changePart(
  draft: IntentDraft,
  index: number,
  patch: Partial<PartDraft>,
): IntentDraft {
  return {
    ...draft,
    parts: draft.parts.map((part, at) =>
      at === index ? { ...part, ...patch } : part,
    ),
  };
}

/** The form with the section `key` changed to `text`. */
export function changeSection(
  draft: IntentDraft,
  key: SectionKey,
  text: string,
): IntentDraft {
  return { ...draft, sections: { ...draft.sections, [key]: text } };
}
