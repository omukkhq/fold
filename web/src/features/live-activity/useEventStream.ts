import { useQuery } from "@tanstack/react-query";
import { listEvents } from "../../api/events";
import { keys } from "../../api/keys";
import { listReviewCheckouts } from "../../api/reviews";
import { useLiveStatus, type LiveStatus } from "../../live/status";
import { useDataClock } from "../../live/useDataClock";
import {
  checkoutRows,
  eventRows,
  type CheckoutRow,
  type EventRow,
} from "./eventStream";

export interface EventStreamView {
  status: "pending" | "error" | "ready";
  rows: EventRow[];
  live: LiveStatus;
  retry: () => void;
}

export interface CheckoutsView {
  status: "pending" | "error" | "ready";
  rows: CheckoutRow[];
  retry: () => void;
}

/** The stored events come from the list read; the stream then adds each new one to the cache, so nothing polls and a cached list never goes stale by time. */
export function useEventStream(slug: string): EventStreamView {
  const live = useLiveStatus();
  const list = useQuery({
    queryKey: keys.events(slug),
    queryFn: () => listEvents(slug),
    staleTime: Infinity,
  });
  const now = useDataClock([list.dataUpdatedAt, live.lastEventAt]);
  return {
    status: list.isError
      ? "error"
      : list.data === undefined
        ? "pending"
        : "ready",
    rows: eventRows(list.data ?? [], now),
    live,
    retry: () => void list.refetch(),
  };
}

export function useReviewCheckouts(slug: string): CheckoutsView {
  const live = useLiveStatus();
  const list = useQuery({
    queryKey: keys.reviewCheckouts(slug),
    queryFn: () => listReviewCheckouts(slug),
    staleTime: Infinity,
  });
  const now = useDataClock([list.dataUpdatedAt, live.lastEventAt]);
  return {
    status: list.isError
      ? "error"
      : list.data === undefined
        ? "pending"
        : "ready",
    rows: checkoutRows(list.data ?? [], now),
    retry: () => void list.refetch(),
  };
}
