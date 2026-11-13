import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { TaskStatus } from "@/api/board";
import { boardCards, boardQuery } from "@/api/board";
import { workersQuery } from "@/api/queries";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { ReconnectingBanner } from "@/components/states/ReconnectingBanner";
import { cn } from "@/lib/cn";
import { useMediaQuery } from "@/lib/pageState";
import { useLiveRetry } from "@/live/retry";
import { useDataClock } from "@/live/useDataClock";
import { useLiveStatus } from "@/live/status";
import {
  BareColumns,
  BoardColumns,
  EmptyColumns,
  LoadingColumns,
} from "./BoardColumns";
import { BoardToolbar } from "./BoardToolbar";
import { groupColumns } from "./columns";
import { applyFilters, labelOptions, needsYouCount } from "./filters";
import { needsOwners, ownerChoices, ownersOf } from "./owners";
import { isFiltered, type BoardSearch } from "./search";
import {
  PHONE_LAYOUT,
  phoneColumn,
  phoneShown,
  reconnectAsOf,
  summaryText,
} from "./view";

export interface BoardScreenProps {
  slug: string;
  search: BoardSearch;
  onSearch: (next: BoardSearch) => void;
}

/**
 * The board is read-only: cards move only because the event stream changed `status` in the Query cache.
 * The task dialog route renders this screen beneath itself, so it takes the filters as props instead of reading the route.
 */
export function BoardScreen({ slug, search, onSearch }: BoardScreenProps) {
  const board = useQuery(boardQuery(slug));
  const workers = useQuery(workersQuery(slug));
  const owners = useMemo(
    () => (workers.data ? ownersOf(workers.data) : null),
    [workers.data],
  );
  const status = useLiveStatus();
  const retry = useLiveRetry();
  // A new reconnect attempt renews the time too, so the reconnect countdown moves.
  const now = useDataClock(
    [board.dataUpdatedAt, status.lastEventAt],
    [status.retryAt],
  );
  const { all, columns } = useMemo(() => {
    const cards = board.data ? boardCards(board.data) : [];
    return {
      all: cards,
      columns: groupColumns(
        applyFilters(cards, search, owners),
        board.data,
        isFiltered(search),
      ),
    };
  }, [board.data, search, owners]);
  // Main agent and the area leads are told by the workers answer, so the board waits for it instead of showing no card.
  const ownersWanted = needsOwners(search.owner);
  const ownersWaiting = ownersWanted && workers.isPending;
  const asOf = reconnectAsOf(status, board.dataUpdatedAt);
  const isPhone = useMediaQuery(PHONE_LAYOUT);
  const [chosen, setChosen] = useState<TaskStatus | null>(null);
  const [opened, setOpened] = useState<TaskStatus | null>(null);
  if (opened === null && board.data !== undefined && !ownersWaiting) {
    setOpened(phoneColumn(columns));
  }
  const phone = isPhone
    ? { selected: phoneShown(chosen, opened, columns), onSelect: setChosen }
    : null;

  let body;
  if (board.isPending || ownersWaiting) {
    body = <LoadingColumns phone={phone} />;
  } else if (board.isError) {
    body = <BareColumns phone={phone} />;
  } else if (all.length === 0) {
    body = <EmptyColumns slug={slug} phone={phone} />;
  } else {
    body = (
      <BoardColumns slug={slug} columns={columns} now={now} phone={phone} />
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {asOf === null ? null : (
        <ReconnectingBanner
          asOf={asOf}
          now={now}
          retryAt={status.retryAt ?? undefined}
          shown="the board"
          onRetry={retry}
        />
      )}
      {board.isError ? (
        <ErrorBanner
          title="The board could not load."
          message="The daemon did not answer."
          onRetry={() => void board.refetch()}
        />
      ) : null}
      {ownersWanted && workers.isError ? (
        <ErrorBanner
          title="The owners could not load."
          message="The daemon did not answer, so the Owner filter matches only human tasks."
          onRetry={() => void workers.refetch()}
        />
      ) : null}
      <BoardToolbar
        search={search}
        onSearch={onSearch}
        labels={labelOptions(all, search.label)}
        owners={ownerChoices(owners, search.owner)}
        needsYou={board.isPending ? null : needsYouCount(all)}
        summary={
          board.isPending || ownersWaiting
            ? "Loading the board…"
            : summaryText(columns)
        }
        phone={isPhone}
      />
      {/* The columns dim while they show the board as of an earlier time. */}
      <div
        className={cn(
          "flex min-h-0 flex-1 flex-col",
          asOf !== null && "opacity-55",
        )}
      >
        {body}
      </div>
    </div>
  );
}
