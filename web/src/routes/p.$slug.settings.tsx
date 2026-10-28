import { createFileRoute } from "@tanstack/react-router";
import { SettingsScreen } from "../features/settings/SettingsScreen";

export const Route = createFileRoute("/p/$slug/settings")({
  component: SettingsRoute,
});

function SettingsRoute() {
  const { slug } = Route.useParams();
  return <SettingsScreen slug={slug} />;
}
