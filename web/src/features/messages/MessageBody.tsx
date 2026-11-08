import { useMemo } from "react";
import { Markdown } from "@/components/text/Markdown";
import { remarkIds } from "./remarkIds";

/** The body as Markdown with linked `#` and `@` mentions and bare ids; HTML in a body shows as typed and never becomes an element. */
export function MessageBody({ body, slug }: { body: string; slug: string }) {
  const plugins = useMemo(() => [remarkIds(slug)], [slug]);
  return (
    <Markdown mentions={{ slug }} plugins={plugins}>
      {body}
    </Markdown>
  );
}
