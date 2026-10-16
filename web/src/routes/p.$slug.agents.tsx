import { createFileRoute } from "@tanstack/react-router";
import { agentsQuery, workersQuery } from "../api/queries";
import { AgentsPending, AgentsScreen } from "../features/agents/AgentsScreen";
import { parseAgentsSearch } from "../features/workers/search";

export const Route = createFileRoute("/p/$slug/agents")({
  validateSearch: parseAgentsSearch,
  loader: async ({ context, params }) => {
    await Promise.all([
      context.queryClient.query({
        ...agentsQuery(params.slug),
        staleTime: "static",
      }),
      context.queryClient.query({
        ...workersQuery(params.slug),
        staleTime: "static",
      }),
    ]);
  },
  pendingComponent: AgentsPending,
  component: AgentsRoute,
});

function AgentsRoute() {
  const { slug } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <AgentsScreen
      slug={slug}
      search={search}
      onSearch={(next) => {
        void navigate({ search: next, replace: true });
      }}
    />
  );
}
