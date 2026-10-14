import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { agentBarQuery, boardQuery } from "@/api/board";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { ReconnectingBanner } from "@/components/states/ReconnectingBanner";
import { cn } from "@/lib/cn";
import { useLiveRetry } from "@/live/retry";
import { useLiveStatus } from "@/live/status";
import {
  BareColumns,
  BoardColumns,
  EmptyColumns,
  LoadingColumns,
} from "./BoardColumns";
import { BoardToolbar } from "./BoardToolbar";
import { boardCards, groupColumns } from "./columns";
import { applyFilters, labelOptions, needsYouCount } from "./filters";
import type { BoardSearch } from "./search";
import { clockTime, reconnectAsOf, summaryText } from "./view";

/**
 * The time the board last rendered new data, for its relative times and the reconnect countdown. It never ticks:
 * the screens run no timer, so the times move when a fetch, an event or a new reconnect attempt arrives.
 */
function useBoardClock(
  dataTimes: readonly (number | null)[],
  retryAt: number | null,
): number {
  const stamp = [...dataTimes, retryAt].map(String).join(":");
  const [clock, setClock] = useState(() => ({
    stamp,
    now: clockTime(dataTimes),
  }));
  if (clock.stamp !== stamp) {
    const next = { stamp, now: clockTime(dataTimes) };
    setClock(next);
    return next.now;
  }
  return clock.now;
}

export interface BoardScreenProps {
  slug: string;
  search: BoardSearch;
  onSearch: (next: BoardSearch) => void;
}

/**
 * W02. The board is read-only: cards move only because the event stream changed `status` in the Query cache.
 * The task dialog route renders this screen beneath itself, so it takes the filters as props instead of reading the route.
 */
export function BoardScreen({ slug, search, onSearch }: BoardScreenProps) {
  const board = useQuery(boardQuery(slug));
  const agents = useQuery(agentBarQuery(slug));
  const status = useLiveStatus();
  const retry = useLiveRetry();
  const now = useBoardClock(
    [board.dataUpdatedAt, status.lastEventAt],
    status.retryAt,
  );
  const all = board.data ? boardCards(board.data) : [];
  const columns = groupColumns(applyFilters(all, search));
  const asOf = reconnectAsOf(status, board.dataUpdatedAt);

  let body;
  if (board.isPending) {
    body = <LoadingColumns />;
  } else if (board.isError) {
    body = <BareColumns />;
  } else if (all.length === 0) {
    body = <EmptyColumns />;
  } else {
    body = <BoardColumns slug={slug} columns={columns} now={now} />;
  }

  return (
    <div className="flex flex-1 flex-col">
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
      {/* W02 dims the columns while they show the board as of an earlier time. */}
      <div
        className={cn("flex flex-1 flex-col", asOf !== null && "opacity-55")}
      >
        {body}
      </div>
    </div>
  );
}
