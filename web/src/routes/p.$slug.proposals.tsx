import { createFileRoute } from "@tanstack/react-router";
import { proposalsQuery } from "../api/queries";
import { parseListSearch } from "../features/proposals/filters";
import {
  ProposalsPending,
  ProposalsScreen,
} from "../features/proposals/ProposalsScreen";

export const Route = createFileRoute("/p/$slug/proposals")({
  validateSearch: parseListSearch,
  loader: async ({ context, params }) => {
    await context.queryClient.query({
      ...proposalsQuery(params.slug),
      staleTime: "static",
    });
  },
  pendingComponent: ProposalsPending,
  component: ProposalsRoute,
});

function ProposalsRoute() {
  const { slug } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <ProposalsScreen
      slug={slug}
      search={search}
      onSearch={(next) => {
        void navigate({ search: parseListSearch({ ...next }), replace: true });
      }}
    />
  );
}
