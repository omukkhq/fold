import { useQuery } from "@tanstack/react-query";
import { createFileRoute, notFound } from "@tanstack/react-router";
import { MissingPanel } from "../app/MissingPanel";
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
  notFoundComponent: ProposalNotFound,
  component: ProposalDetailRoute,
});

function ProposalDetailRoute() {
  const { slug, id } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const session = useQuery(sessionQuery());
  return (
    <ProposalDetailScreen
      key={id}
      slug={slug}
      id={id}
      search={search}
      human={session.data?.human ?? null}
      onSearch={(next, history = "replace") => {
        void navigate({
          search: parseDetailSearch({ ...next }),
          replace: history === "replace",
        });
      }}
    />
  );
}

function ProposalNotFound() {
  const { slug, id } = Route.useParams();
  return <MissingPanel slug={slug} missing={{ kind: "proposal", id: id }} />;
}
