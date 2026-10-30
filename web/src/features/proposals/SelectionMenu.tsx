import {
  FilePlus2,
  MessageCircleQuestion,
  MessageSquarePlus,
} from "lucide-react";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import type { LineMenu, MenuNote } from "./lineQuestions";
import { detailHref } from "./view";

function Note({
  slug,
  id,
  note,
}: {
  slug: string;
  id: string;
  note: MenuNote;
}) {
  return (
    <p role="status" className="text-13 text-text-2">
      {note.text}
      {note.link ? (
        <>
          {" "}
          <HrefLink
            href={detailHref(slug, id, note.link.search)}
            className={cn("rounded-s text-accent underline", focusRing)}
          >
            {note.link.label}
          </HrefLink>
        </>
      ) : null}
    </p>
  );
}

interface SelectionMenuProps {
  slug: string;
  /** The proposal the notes link into. */
  id: string;
  menu: LineMenu;
  onAsk: () => void;
  onComment: () => void;
  onRequest: () => void;
}

/** What the human does with selected lines: ask the author or start a review comment, which stay with the review, or make a request for the main agent, which stands apart from them. A selection that takes none shows only why. */
export function SelectionMenu({
  slug,
  id,
  menu,
  onAsk,
  onComment,
  onRequest,
}: SelectionMenuProps) {
  const notes = menu.notes.map((note) => (
    <Note key={note.text} slug={slug} id={id} note={note} />
  ));
  if (menu.ask === null && menu.comment === null && menu.request === null) {
    return <div className="flex flex-col gap-4">{notes}</div>;
  }
  return (
    <div className="flex flex-col gap-8">
      <div
        role="group"
        aria-label="Selected lines"
        className="flex flex-wrap items-center gap-8"
      >
        <Button
          variant="primary"
          size="sm"
          disabled={menu.ask === null}
          icon={<MessageCircleQuestion aria-hidden="true" size={14} />}
          onClick={onAsk}
        >
          Ask the author
        </Button>
        <Button
          size="sm"
          disabled={menu.comment === null}
          icon={<MessageSquarePlus aria-hidden="true" size={14} />}
          onClick={onComment}
        >
          Review comment
        </Button>
        <span aria-hidden="true" className="mx-4 h-20 w-px bg-line" />
        <Button
          variant="quiet"
          size="sm"
          disabled={menu.request === null}
          icon={<FilePlus2 aria-hidden="true" size={14} />}
          onClick={onRequest}
        >
          Make a request
        </Button>
      </div>
      {notes}
    </div>
  );
}
