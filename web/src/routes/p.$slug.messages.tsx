import { createFileRoute } from "@tanstack/react-router";
import { messagesQuery } from "../api/queries";
import {
  MessagesPending,
  MessagesScreen,
} from "../features/messages/MessagesScreen";
import { normalizeSearch } from "../features/messages/search";

export const Route = createFileRoute("/p/$slug/messages")({
  validateSearch: normalizeSearch,
  loader: async ({ context, params }) => {
    await context.queryClient.query({
      ...messagesQuery(params.slug),
      staleTime: "static",
    });
  },
  pendingComponent: MessagesPending,
  component: MessagesRoute,
});

function MessagesRoute() {
  const { slug } = Route.useParams();
  const search = Route.useSearch();
  return <MessagesScreen slug={slug} search={search} />;
}
