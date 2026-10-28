import { ReconnectingBanner } from "../../components/states/ReconnectingBanner";
import { EmptyPanel } from "../../components/states/EmptyPanel";
import { ErrorBanner } from "../../components/states/ErrorBanner";
import { Skeleton } from "../../components/states/Skeleton";
import { EventStream } from "./EventStream";
import { ReviewCheckouts } from "./ReviewCheckouts";
import { shownAsOf } from "./liveStates";
import { useLiveActivity } from "./useLiveActivity";
import { WorkspacePanel } from "./WorkspacePanel";
import { LegendItem } from "./Zones";

export function LiveActivityScreen({ slug }: { slug: string }) {
  const activity = useLiveActivity(slug);
  const { live, now } = activity;
  return (
    <>
      {live.state === "reconnecting" ? (
        <ReconnectingBanner now={now} asOf={shownAsOf(live, now)} />
      ) : null}
      {activity.status === "error" ? (
        <ErrorBanner
          title="Activity could not load."
          message="The workspaces did not arrive."
          onRetry={activity.retry}
        />
      ) : null}
      <div className="flex flex-col gap-24 px-24 pt-20 pb-24 lg:flex-row">
        <div className="flex min-w-0 flex-1 flex-col gap-16">
          <div className="flex flex-wrap items-end justify-between gap-14">
            <div className="flex flex-col gap-4">
              <h1 className="text-24 font-semibold text-text">Activity</h1>
              <p className="text-15 text-text-2">
                What every worker is doing now. Working changes are not in main
                until a proposal is integrated.
              </p>
            </div>
            <ul aria-label="Legend" className="flex items-center gap-14">
              <LegendItem tone="working">Working, not recorded</LegendItem>
              <LegendItem tone="recorded">Recorded changeset</LegendItem>
              <LegendItem tone="main">Main</LegendItem>
            </ul>
          </div>
          {activity.status === "pending" ? (
            <>
              <Skeleton />
              <Skeleton />
            </>
          ) : null}
          {activity.status === "ready" && activity.panels.length === 0 ? (
            <EmptyPanel title="No active workspaces">
              A workspace appears here when a worker claims a task and starts to
              edit.
            </EmptyPanel>
          ) : null}
          {activity.panels.map((panel) => (
            <WorkspacePanel
              key={panel.entry.id}
              slug={slug}
              panel={panel}
              now={now}
            />
          ))}
          <ReviewCheckouts slug={slug} />
        </div>
        <EventStream slug={slug} />
      </div>
    </>
  );
}
