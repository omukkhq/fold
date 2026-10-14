export interface FieldIds {
  input: string;
  hint: string;
  error: string;
}

export function fieldIds(base: string): FieldIds {
  return { input: base, hint: `${base}-hint`, error: `${base}-error` };
}

/** The ids of the hint and error that are shown, in reading order; undefined when there are none. */
export function describedBy(
  ids: FieldIds,
  shown: { hint?: boolean; error?: boolean },
): string | undefined {
  const parts = [
    shown.hint ? ids.hint : "",
    shown.error ? ids.error : "",
  ].filter(Boolean);
  return parts.length > 0 ? parts.join(" ") : undefined;
}
