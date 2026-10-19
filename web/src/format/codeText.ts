/** One run of a sentence: plain words, or a command drawn as small inline code. */
export type CodePart = string | { code: string };

const MARK = "`";
const ESCAPE = "\\";

/** Splits a sentence that marks each command between single backticks. A backslash before a backtick or a backslash makes it a plain character, as in Markdown. A backtick without its pair stays a plain character, and an empty pair shows nothing. */
export function codeParts(text: string): CodePart[] {
  const runs: string[] = [];
  let current = "";
  for (let at = 0; at < text.length; at += 1) {
    const char = text.charAt(at);
    const next = text.charAt(at + 1);
    if (char === ESCAPE && (next === MARK || next === ESCAPE)) {
      current += next;
      at += 1;
    } else if (char === MARK) {
      runs.push(current);
      current = "";
    } else {
      current += char;
    }
  }
  runs.push(current);
  const parts: CodePart[] = [];
  let plain = "";
  runs.forEach((run, index) => {
    if (index % 2 === 0) {
      plain += run;
    } else if (index === runs.length - 1) {
      plain += MARK + run;
    } else if (run !== "") {
      if (plain !== "") parts.push(plain);
      plain = "";
      parts.push({ code: run });
    }
  });
  if (plain !== "") parts.push(plain);
  return parts;
}

/** Data, such as a path, that stands in a sentence with marked commands: its backticks and backslashes are escaped, so they never pair with a command's marks. */
export function codeData(value: string): string {
  return value
    .replaceAll(ESCAPE, ESCAPE + ESCAPE)
    .replaceAll(MARK, ESCAPE + MARK);
}
