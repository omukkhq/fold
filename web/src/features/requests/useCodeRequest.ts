import { useQueryClient } from "@tanstack/react-query";
import { keys } from "@/api/keys";
import { createCodeRequest } from "@/api/requests";
import type { DiffSelection } from "@/features/diff/selection";
import { codeRequestBody, type CodeAnchor } from "./codeLocation";

/** Makes a request about the selected lines and resolves with its id once the request list holds it, so the lines show their mark. */
export function useCodeRequest(
  slug: string,
): (
  anchor: CodeAnchor,
  selection: DiffSelection,
  body: string,
) => Promise<string> {
  const client = useQueryClient();
  return async (anchor, selection, body) => {
    const made = await createCodeRequest(
      slug,
      codeRequestBody(anchor, selection, body),
    );
    await client.invalidateQueries({ queryKey: keys.requests(slug) });
    return made.request.id;
  };
}
