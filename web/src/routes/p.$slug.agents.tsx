import { createFileRoute } from "@tanstack/react-router";
import { agentsQuery } from "../api/queries";
import { AgentsPending, AgentsScreen } from "../features/agents/AgentsScreen";

export const Route = createFileRoute("/p/$slug/agents")({
  loader: async ({ context, params }) => {
    await context.queryClient.query({
      ...agentsQuery(params.slug),
      staleTime: "static",
    });
  },
  pendingComponent: AgentsPending,
  component: AgentsRoute,
});

function AgentsRoute() {
  const { slug } = Route.useParams();
  return <AgentsScreen slug={slug} />;
}
