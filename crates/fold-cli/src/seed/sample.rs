use std::io;
use std::path::Path;

pub const README: &str = "# Notes service

A small service that keeps notes for a team. It has a list view, a detail view and a JSON API.

## Layout

- `src/app.js` starts the server.
- `src/routes.js` maps requests to handlers.
- `src/db.js` holds the data.
- `src/format.js` formats values for display.
";

pub const APP: &str = "import { routes } from \"./routes.js\";
import { openDatabase } from \"./db.js\";

export async function start(port) {
  const db = await openDatabase();
  const handler = routes(db);
  const server = createServer(handler);
  server.listen(port);
  console.log(`listening on ${port}`);
  return server;
}

function createServer(handler) {
  return { listen: (port) => handler.bind(port) };
}
";

pub const ROUTES: &str = "export function routes(db) {
  return {
    \"GET /health\": () => ({ status: 200, body: \"ok\" }),
    \"GET /items\": async () => ({ status: 200, body: await db.all(\"items\") }),
  };
}

export function notFound() {
  return { status: 404, body: \"not found\" };
}

export function methodNotAllowed() {
  return { status: 405, body: \"method not allowed\" };
}
";

pub const DB: &str = "// A tiny in-memory store standing in for the real database.

const tables = new Map();

export async function openDatabase() {
  tables.set(\"items\", []);
  return {
    all: async (name) => tables.get(name) ?? [],
    insert: async (name, row) => tables.get(name).push(row),
  };
}
";

pub const FORMAT: &str = "// Formatting helpers shared by the list and the detail views.

export function formatDate(value) {
  return new Date(value).toISOString();
}

export function formatCount(count) {
  return String(count);
}

export function formatName(user) {
  return user.name;
}
";

pub const ARCHITECTURE: &str = "# Architecture

The service is one process. Requests reach `routes`, which reads from the in-memory store and returns a plain object. The server turns that object into an HTTP response.

Nothing is written to disk. A restart starts with an empty store.
";

pub const CHANGELOG: &str = "# Changelog

## 0.1.0

- First version with the health check and the item list.
";

/// The files of the sample application, in the order they are written.
pub const FILES: [(&str, &str); 7] = [
    ("README.md", README),
    ("src/app.js", APP),
    ("src/routes.js", ROUTES),
    ("src/db.js", DB),
    ("src/format.js", FORMAT),
    ("docs/architecture.md", ARCHITECTURE),
    ("docs/changelog.md", CHANGELOG),
];

pub fn write(root: &Path) -> io::Result<()> {
    for (path, content) in FILES {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
    }
    Ok(())
}
