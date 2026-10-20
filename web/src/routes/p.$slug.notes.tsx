import { createFileRoute } from "@tanstack/react-router";
import { notesQuery, notesUnreadQuery } from "../api/queries";
import { NotesPending, NotesScreen } from "../features/notes/NotesScreen";
import { parseNotesSearch } from "../features/notes/notes";

export const Route = createFileRoute("/p/$slug/notes")({
  validateSearch: parseNotesSearch,
  loader: async ({ context, params }) => {
    await Promise.all([
      context.queryClient.query({
        ...notesQuery(params.slug),
        staleTime: "static",
      }),
      context.queryClient.query({
        ...notesUnreadQuery(params.slug),
        staleTime: "static",
      }),
    ]);
  },
  pendingComponent: NotesPending,
  component: NotesRoute,
});

function NotesRoute() {
  const { slug } = Route.useParams();
  const search = Route.useSearch();
  return <NotesScreen slug={slug} search={search} />;
}
