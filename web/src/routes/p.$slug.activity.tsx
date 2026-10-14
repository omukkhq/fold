import { createFileRoute } from "@tanstack/react-router";
import { LiveActivityScreen } from "../features/live-activity/LiveActivityScreen";

export const Route = createFileRoute("/p/$slug/activity")({
  component: Activity,
});

function Activity() {
  const { slug } = Route.useParams();
  return <LiveActivityScreen slug={slug} />;
}
