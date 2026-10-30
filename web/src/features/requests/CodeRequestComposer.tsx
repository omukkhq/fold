import { useQuery } from "@tanstack/react-query";
import { FilePlus2 } from "lucide-react";
import { useState } from "react";
import { sessionQuery } from "@/api/queries";
import { Button } from "@/components/button/Button";
import type { DiffSelection } from "@/features/diff/selection";
import {
  REQUEST_NAME_NOTE,
  makeRequestTitle,
  type CodeAnchor,
} from "./codeLocation";
import { MakeRequestBox } from "./MakeRequestBox";
import { useCodeRequest } from "./useCodeRequest";

/** What lines of main show once selected: one action that makes a request about them, then its box. Without a name only the reason shows. */
export function CodeRequestComposer({
  slug,
  anchor,
  selection,
  focusLine,
  onDone,
}: {
  slug: string;
  anchor: CodeAnchor;
  selection: DiffSelection;
  /** Puts focus back on the selected line as the box closes. */
  focusLine: () => void;
  /** Clears the selection once the request is made or the box is closed. */
  onDone: () => void;
}) {
  const session = useQuery(sessionQuery());
  const make = useCodeRequest(slug);
  const [open, setOpen] = useState(false);
  const human = session.data?.human ?? null;
  if (human === null) {
    return (
      <p role="status" className="text-13 text-text-2">
        {REQUEST_NAME_NOTE}
      </p>
    );
  }
  if (!open) {
    return (
      <div role="group" aria-label="Selected lines" className="flex">
        <Button
          size="sm"
          icon={<FilePlus2 aria-hidden="true" size={14} />}
          onClick={() => {
            setOpen(true);
          }}
        >
          Make a request
        </Button>
      </div>
    );
  }
  return (
    <MakeRequestBox
      slug={slug}
      title={makeRequestTitle(anchor, selection)}
      onCancel={() => {
        focusLine();
        setOpen(false);
      }}
      onDone={() => {
        focusLine();
        onDone();
      }}
      onSubmit={(body) => make(anchor, selection, body)}
    />
  );
}
