import { createFileRoute, notFound } from "@tanstack/react-router";
import { BoardScreen } from "../features/board/BoardScreen";
import { boardSearchSchema, cleanSearch } from "../features/board/search";
import { TaskDialog } from "../features/task/TaskDialog";
import { parseTaskParam } from "./params";

/** The task dialog over the board. It takes the board's filters as its own search, so closing returns to the same board. */
export const Route = createFileRoute("/p/$slug/t/$task")({
  validateSearch: boardSearchSchema,
  beforeLoad: ({ params }) => {
    if (parseTaskParam(params.task) === null) {
      notFound({ throw: true });
    }
  },
  component: TaskRoute,
});

function TaskRoute() {
  const { slug, task } = Route.useParams();
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
      <TaskDialog
        key={task}
        slug={slug}
        task={task}
        onClose={() => {
          void navigate({
            to: "/p/$slug/board",
            params: { slug },
            search: cleanSearch(search),
          });
        }}
      />
    </>
  );
}
