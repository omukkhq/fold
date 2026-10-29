import { MessageCircleQuestion, MessageSquarePlus } from "lucide-react";
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
}

/** What the human does with selected lines: ask the author, or start a review comment. A selection that takes neither shows only why. */
export function SelectionMenu({
  slug,
  id,
  menu,
  onAsk,
  onComment,
}: SelectionMenuProps) {
  const notes = menu.notes.map((note) => (
    <Note key={note.text} slug={slug} id={id} note={note} />
  ));
  if (menu.ask === null && menu.comment === null) {
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
      </div>
      {notes}
    </div>
  );
}
