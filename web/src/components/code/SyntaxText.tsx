import { Fragment, useMemo } from "react";
import type { SyntaxLine } from "@/syntax/highlight";
import type { Language } from "@/syntax/languages";
import { highlightRequest, useHighlight } from "@/syntax/useHighlight";

/** One line's tokens; a token in the line's own colour is plain text. */
export function SyntaxTokens({ tokens }: { tokens: SyntaxLine }) {
  return tokens.map((token, i) =>
    token.color === null ? (
      <Fragment key={i}>{token.content}</Fragment>
    ) : (
      <span key={i} style={{ color: token.color }}>
        {token.content}
      </span>
    ),
  );
}

/** Code as plain text at once, coloured once its grammar has tokenized it. The caller draws the `pre` or `code` around it. */
export function SyntaxText({
  code,
  language,
}: {
  code: string;
  language: Language | null;
}) {
  const lines = useMemo(() => code.split(/\r?\n/), [code]);
  const request = useMemo(
    () => highlightRequest(language, [lines]),
    [language, lines],
  );
  const tokens = useHighlight(request)?.[0];
  if (!tokens) return code;
  return lines.map((_, i) => (
    <Fragment key={i}>
      {i > 0 ? "\n" : null}
      <SyntaxTokens tokens={tokens[i] ?? []} />
    </Fragment>
  ));
}
