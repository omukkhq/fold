import { useEffect, useEffectEvent, useState } from "react";
import { highlightSegments, type SyntaxLine } from "./highlight";
import type { LanguageSource } from "./languages";

/** Code to highlight. `key` names the language and text, so a new object with the same code starts no second highlight. */
export interface HighlightRequest {
  key: string;
  language: LanguageSource | null;
  segments: readonly (readonly string[])[];
}

/** A request whose key is built from its language and text. */
export function highlightRequest(
  language: LanguageSource | null,
  segments: readonly (readonly string[])[],
): HighlightRequest {
  const text = segments.map((s) => s.join("\n")).join("\u0000");
  return { key: `${language ?? ""}\u0000${text}`, language, segments };
}

/**
 * The tokens of each segment once they are ready; null until then, and for code that stays plain. A changed request
 * stops the work for the old one, and the old tokens are never shown for the new code.
 */
export function useHighlight(
  request: HighlightRequest | null,
): SyntaxLine[][] | null {
  const [result, setResult] = useState<{
    key: string;
    tokens: SyntaxLine[][] | null;
  } | null>(null);
  const key = request?.language ? request.key : null;
  const start = useEffectEvent((signal: AbortSignal) =>
    request
      ? highlightSegments(request.language, request.segments, { signal })
      : Promise.resolve(null),
  );
  useEffect(() => {
    if (key === null) return;
    const controller = new AbortController();
    void start(controller.signal).then((tokens) => {
      if (!controller.signal.aborted) setResult({ key, tokens });
    });
    return () => {
      controller.abort();
    };
  }, [key]);
  return result !== null && result.key === key ? result.tokens : null;
}
