import { createFileRoute } from "@tanstack/react-router";
import { ChatScreen } from "../features/chat/ChatScreen";
import { parseChatSearch } from "../features/chat/chat";

export const Route = createFileRoute("/p/$slug/agents_/$agent/chat")({
  validateSearch: parseChatSearch,
  component: ChatRoute,
});

function ChatRoute() {
  const { slug, agent } = Route.useParams();
  const { note } = Route.useSearch();
  return <ChatScreen key={agent} slug={slug} agent={agent} note={note} />;
}
