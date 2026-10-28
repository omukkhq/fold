import { useSuspenseQuery } from "@tanstack/react-query";
import { createFileRoute, notFound } from "@tanstack/react-router";
import { MissingPanel } from "../app/MissingPanel";
import { comparisonQuery } from "../api/queries";
import { DiffScreen } from "../features/diff/DiffScreen";
import { namesComparison, parseDiffSearch } from "../features/diff/search";

export const Route = createFileRoute("/p/$slug/diff")({
  validateSearch: parseDiffSearch,
  beforeLoad: ({ search }) => {
    if (!namesComparison(search)) {
      notFound({ throw: true });
    }
  },
  loaderDeps: ({ search }) => ({ from: search.from, to: search.to }),
  loader: async ({ context, params, deps }) => {
    await context.queryClient.query(
      comparisonQuery(params.slug, deps.from, deps.to),
    );
  },
  notFoundComponent: DiffNotFound,
  component: DiffRoute,
});

function DiffRoute() {
  const { slug } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const { data, dataUpdatedAt } = useSuspenseQuery(
    comparisonQuery(slug, search.from, search.to),
  );
  return (
    <DiffScreen
      slug={slug}
      search={search}
      comparison={data}
      readAt={dataUpdatedAt}
      onSelectFile={(path) => {
        void navigate({ search: (old) => ({ ...old, path }), replace: true });
      }}
    />
  );
}

function DiffNotFound() {
  const { slug } = Route.useParams();
  return <MissingPanel slug={slug} missing={{ kind: "page" }} />;
}
