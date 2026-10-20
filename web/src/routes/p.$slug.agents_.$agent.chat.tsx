import { createFileRoute } from "@tanstack/react-router";
import { ChatScreen } from "../features/chat/ChatScreen";

export const Route = createFileRoute("/p/$slug/agents_/$agent/chat")({
  component: ChatRoute,
});

function ChatRoute() {
  const { slug, agent } = Route.useParams();
  return <ChatScreen key={agent} slug={slug} agent={agent} />;
}
