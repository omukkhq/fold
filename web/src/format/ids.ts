export type IdKind =
  | "task"
  | "proposal"
  | "changeset"
  | "revision"
  | "workspace"
  | "checkpoint"
  | "message"
  | "request";

export interface FoundId {
  kind: IdKind;
  number: number;
  /** The id as written, for example `T-12`. */
  text: string;
  /** Offset of the first character in the searched text. */
  start: number;
  /** Offset one past the last character. */
  end: number;
}

const PREFIXES: readonly (readonly [string, IdKind])[] = [
  ["T-", "task"],
  ["P-", "proposal"],
  ["C-", "changeset"],
  ["ws-", "workspace"],
  ["ck-", "checkpoint"],
  ["m-", "message"],
  ["RQ-", "request"],
  ["r", "revision"],
];

// A number has no leading zero and fits the daemon's 32-bit counters; only a revision can be 0, the genesis `r0`. The guards on both sides keep an id
// inside a longer word (`xT-12`, `T-12a`, `rc-3`, `a-T-12`, `T-12-a`) from counting as one.
const ID = new RegExp(
  `(?<![A-Za-z0-9_-])(${PREFIXES.map(([prefix]) => prefix).join("|")})([1-9][0-9]{0,9}|0)(?![A-Za-z0-9_]|-[A-Za-z0-9_])`,
  "g",
);

const MAX_NUMBER = 4_294_967_295;

function toFound(match: RegExpExecArray): FoundId | null {
  const prefix = match[1];
  const digits = match[2];
  const entry = PREFIXES.find(([candidate]) => candidate === prefix);
  if (!entry || digits === undefined) {
    return null;
  }
  const number = Number(digits);
  if (number > MAX_NUMBER || (number === 0 && entry[1] !== "revision")) {
    return null;
  }
  return {
    kind: entry[1],
    number,
    text: match[0],
    start: match.index,
    end: match.index + match[0].length,
  };
}

/** Every public id in `text`, in order, with exact spans. */
export function findIds(text: string): FoundId[] {
  const found: FoundId[] = [];
  for (const match of text.matchAll(ID)) {
    const id = toFound(match);
    if (id) {
      found.push(id);
    }
  }
  return found;
}

/** The id when the whole string is exactly one public id. */
export function parseId(text: string): FoundId | null {
  const [only] = findIds(text);
  if (only?.start === 0 && only.end === text.length) {
    return only;
  }
  return null;
}

/** One run of a sentence: plain words, or a public id. */
export interface IdRun {
  text: string;
  id: boolean;
}

/** Splits `text` into its public ids and the words between them, so a screen can keep each id on one line: a browser breaks a line after the hyphen of `T-7`. */
export function idRuns(text: string): IdRun[] {
  const runs: IdRun[] = [];
  let at = 0;
  for (const found of findIds(text)) {
    if (found.start > at) {
      runs.push({ text: text.slice(at, found.start), id: false });
    }
    runs.push({ text: found.text, id: true });
    at = found.end;
  }
  if (at < text.length) {
    runs.push({ text: text.slice(at), id: false });
  }
  return runs;
}
