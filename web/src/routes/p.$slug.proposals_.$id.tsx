import { useQuery } from "@tanstack/react-query";
import { createFileRoute, notFound } from "@tanstack/react-router";
import { proposalQuery, sessionQuery } from "../api/queries";
import {
  ProposalDetailPending,
  ProposalDetailScreen,
} from "../features/proposals/ProposalDetailScreen";
import { parseDetailSearch } from "../features/proposals/view";
import { parseProposalParam } from "./params";

export const Route = createFileRoute("/p/$slug/proposals_/$id")({
  validateSearch: parseDetailSearch,
  beforeLoad: ({ params }) => {
    if (parseProposalParam(params.id) === null) {
      notFound({ throw: true });
    }
  },
  loader: async ({ context, params }) => {
    await context.queryClient.query({
      ...proposalQuery(params.slug, params.id),
      staleTime: "static",
    });
  },
  pendingComponent: ProposalDetailPending,
  component: ProposalDetailRoute,
});

function ProposalDetailRoute() {
  const { slug, id } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const session = useQuery(sessionQuery());
  return (
    <ProposalDetailScreen
      slug={slug}
      id={id}
      search={search}
      human={session.data?.humanName ?? null}
      onSearch={(next) => {
        void navigate({
          search: parseDetailSearch({ ...next }),
          replace: true,
        });
      }}
    />
  );
}
