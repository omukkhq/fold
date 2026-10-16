export type CopyState = "idle" | "copied" | "failed";

/** Writes `text` to the clipboard and says what happened; a browser without clipboard access, or one that refuses the write, is `failed`. */
export async function copyOutcome(
  text: string,
  clipboard: Pick<Clipboard, "writeText"> | undefined,
): Promise<CopyState> {
  if (!clipboard) {
    return "failed";
  }
  try {
    await clipboard.writeText(text);
    return "copied";
  } catch {
    return "failed";
  }
}
