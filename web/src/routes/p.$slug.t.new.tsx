import { createFileRoute } from "@tanstack/react-router";
import { BoardScreen } from "../features/board/BoardScreen";
import { boardSearchSchema, cleanSearch } from "../features/board/search";
import { NewTaskDialog } from "../features/task/NewTaskDialog";

/** The task dialog's create form over the board; a created task opens in the same dialog. */
export const Route = createFileRoute("/p/$slug/t/new")({
  validateSearch: boardSearchSchema,
  component: NewTaskRoute,
});

function NewTaskRoute() {
  const { slug } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <>
      <BoardScreen
        slug={slug}
        search={search}
        onSearch={(next) => {
          void navigate({ search: cleanSearch(next), replace: true });
        }}
      />
      <NewTaskDialog
        slug={slug}
        onClose={() => {
          void navigate({
            to: "/p/$slug/board",
            params: { slug },
            search: cleanSearch(search),
          });
        }}
        onCreated={(task) => {
          void navigate({
            to: "/p/$slug/t/$task",
            params: { slug, task },
            search: cleanSearch(search),
            replace: true,
          });
        }}
      />
    </>
  );
}
