import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { agentBarQuery } from "@/api/agentBar";
import { boardCards, boardQuery } from "@/api/board";
import { PausedBanner } from "@/app/PausedBanner";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { ReconnectingBanner } from "@/components/states/ReconnectingBanner";
import { cn } from "@/lib/cn";
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
import { isFiltered, type BoardSearch } from "./search";
import { reconnectAsOf, summaryText } from "./view";

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
  const agents = useQuery(agentBarQuery(slug));
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
        applyFilters(cards, search),
        board.data,
        isFiltered(search),
      ),
    };
  }, [board.data, search]);
  const asOf = reconnectAsOf(status, board.dataUpdatedAt);

  let body;
  if (board.isPending) {
    body = <LoadingColumns />;
  } else if (board.isError) {
    body = <BareColumns />;
  } else if (all.length === 0) {
    body = <EmptyColumns slug={slug} />;
  } else {
    body = <BoardColumns slug={slug} columns={columns} now={now} />;
  }

  return (
    <div className="flex flex-1 flex-col">
      <PausedBanner slug={slug} />
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
      <BoardToolbar
        search={search}
        onSearch={onSearch}
        labels={labelOptions(all)}
        agents={agents.data ?? []}
        needsYou={board.isPending ? null : needsYouCount(all)}
        summary={board.isPending ? "Loading the board…" : summaryText(columns)}
      />
      {/* The columns dim while they show the board as of an earlier time. */}
      <div
        className={cn("flex flex-1 flex-col", asOf !== null && "opacity-55")}
      >
        {body}
      </div>
    </div>
  );
}
