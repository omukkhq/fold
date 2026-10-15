import { daemon, ok } from "./daemon";
import { boardCard, boardView } from "./payloads";

/** The board, repository and tree reads the mention popover makes: one task `T-9`, one file `web/report.tsx` and one workspace file `draft.ts`. */
export function mentionRoutes(): void {
  daemon.on("GET /api/v1/projects/:slug/board", () =>
    ok(boardView([boardCard("T-9", { title: "Weekly report" })])),
  );
  daemon.on(
    "GET /api/v1/projects/:slug/workspaces/:id/tree",
    ok({
      path: "",
      entries: [{ name: "draft.ts", path: "draft.ts", kind: "file" }],
    }),
  );
  daemon.on(
    "GET /api/v1/projects/:slug/repository",
    ok({ head: "r3", main_path: "/tmp/main", recent: [] }),
  );
  daemon.on("GET /api/v1/projects/:slug/revisions/:r/tree", (call) =>
    ok({
      path: "",
      entries: call.path.includes("path=web")
        ? [
            {
              name: "report.tsx",
              path: "web/report.tsx",
              kind: "file",
              hash: "h",
              size: 1,
            },
          ]
        : [
            {
              name: "web",
              path: "web",
              kind: "directory",
              hash: "h",
              size: null,
            },
          ],
    }),
  );
}
