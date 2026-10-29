import { HrefLink } from "@/app/HrefLink";
import { Count } from "@/components/badge/Chip";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ActorText } from "@/people/ActorText";
import { PersonName } from "@/people/PersonName";
import type { AgentChat } from "./chats";
import { rowTime } from "./days";
import { chatRow, questionsLabel, unreadLabel } from "./threadRow";

interface ChatListProps {
  chats: AgentChat[];
  selected: string | null;
  now: number;
  you: string | null;
  /** The asks that wait for the signed-in human, as Needs you lists them. */
  open: ReadonlySet<string>;
  hrefOf: (chat: AgentChat) => string;
}

/** The chats as two-line rows that fills separate: the agent and the time of the newest message, then that message and the counts of what waits for you. */
export function ChatList({
  chats,
  selected,
  now,
  you,
  open,
  hrefOf,
}: ChatListProps) {
  return (
    <ul aria-label="Chats" className="flex flex-col gap-6 lg:gap-2">
      {chats.map((chat) => {
        const current = chat.key === selected;
        const row = chatRow(chat, you, open);
        return (
          <li key={chat.key}>
            <HrefLink
              href={hrefOf(chat)}
              aria-current={current ? "true" : undefined}
              className={cn(
                "grid grid-cols-[minmax(0,1fr)_auto] gap-x-12 gap-y-3 rounded-m px-12 py-10",
                current ? "bg-accent-soft" : "max-lg:bg-surface lg:hover:bg-bg",
                focusRing,
              )}
            >
              <span className="truncate font-title text-16 leading-tight font-semibold text-text">
                {"agent" in row.title ? (
                  <PersonName name={row.title.agent} />
                ) : (
                  row.title.text
                )}
              </span>
              <span className="self-baseline text-13 whitespace-nowrap text-text-2 tabular-nums">
                {rowTime(now, chat.last_at)}
              </span>
              <span className="min-w-0 truncate text-14 text-text">
                <ActorText
                  segments={row.preview}
                  personClassName="text-14"
                  you={you}
                />
              </span>
              <span className="flex items-center gap-6 self-center">
                {row.questions > 0 ? (
                  <Count
                    value={row.questions}
                    label={questionsLabel(row.questions)}
                  />
                ) : null}
                {row.unread > 0 ? (
                  <Count
                    value={row.unread}
                    tone="accent"
                    label={unreadLabel(row.unread)}
                  />
                ) : null}
              </span>
            </HrefLink>
          </li>
        );
      })}
    </ul>
  );
}
