#!/usr/bin/env python3
"""Rank the hotspot files of a fold project: churn in the newest revisions of main times size, with the files of reviewer fixes counted beside them.

History and reviewer fixes come only from fold's read tools (get_repository_state, get_revision, get_proposal, compare_revisions), called through `fld mcp` as the agent of the calling session. Sizes are the line counts of the files under --root, a workspace or a checkout. Tests and generated files are left out.

Usage: scripts/hotspots.py [--revisions N] [--top K] [--root DIR]
       scripts/hotspots.py --fixture FILE   (rank recorded tool results; no daemon)
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from collections.abc import Callable, Iterable
from dataclasses import dataclass

GENERATED = (
    re.compile(r"(^|/)web/src/api/schema\.d\.ts$"),
    re.compile(r"(^|/)\.sqlx/"),
    re.compile(r"(^|/)(Cargo\.lock|bun\.lock|package-lock\.json|pnpm-lock\.yaml)$"),
    re.compile(r"(^|/)(dist|target|node_modules)/"),
)

TESTS = (
    re.compile(r"(^|/)tests?/"),
    re.compile(r"(^|/)__tests__/"),
    re.compile(r"(^|/)tests?\.rs$"),
    re.compile(r"_tests?\.(rs|py)$"),
    re.compile(r"(^|/)test_[^/]*\.py$"),
    re.compile(r"\.(test|spec)\.[cm]?[jt]sx?$"),
)


def excluded(path: str) -> str | None:
    """The reason a path is left out of the ranking, or None to rank it."""
    if any(p.search(path) for p in GENERATED):
        return "generated"
    if any(p.search(path) for p in TESTS):
        return "test"
    return None


@dataclass
class Row:
    path: str
    lines: int
    changes: int
    fixes: int

    @property
    def score(self) -> int:
        return self.changes * self.lines


Call = Callable[[str, dict[str, object]], dict]


def collect(call: Call, revisions: int) -> tuple[dict[str, int], dict[str, int], list[str]]:
    """Walk main from its head through `revisions` revisions; return changes per path, reviewer fixes per path and the revisions read."""
    head = call("get_repository_state", {})["head"]
    changes: dict[str, int] = {}
    fixes: dict[str, int] = {}
    seen: list[str] = []
    proposals: list[str] = []
    r: str | None = head
    while r and len(seen) < revisions:
        rev = call("get_revision", {"r": r})
        seen.append(r)
        for f in rev.get("files", []):
            changes[f["path"]] = changes.get(f["path"], 0) + 1
        if rev.get("proposal") and rev["proposal"] not in proposals:
            proposals.append(rev["proposal"])
        r = rev.get("parent")
    for p in proposals:
        versions = call("get_proposal", {"id": p}).get("versions", [])
        for prev, cur in zip(versions, versions[1:]):
            if not cur.get("reviewer_fix"):
                continue
            diff = call("compare_revisions", {"from": prev["changeset"], "to": cur["changeset"]})
            for f in diff.get("files", []):
                fixes[f["path"]] = fixes.get(f["path"], 0) + 1
    return changes, fixes, seen


def rank(changes: dict[str, int], fixes: dict[str, int], lines: Callable[[str], int | None]) -> tuple[list[Row], dict[str, str]]:
    """Rows sorted by score, then reviewer fixes, then path; and the paths left out with the reason."""
    rows: list[Row] = []
    left_out: dict[str, str] = {}
    for path, n in changes.items():
        reason = excluded(path)
        if reason:
            left_out[path] = reason
            continue
        size = lines(path)
        if size is None:
            left_out[path] = "absent"
            continue
        rows.append(Row(path, size, n, fixes.get(path, 0)))
    rows.sort(key=lambda row: (-row.score, -row.fixes, row.path))
    return rows, left_out


def table(rows: Iterable[Row]) -> str:
    out = ["| # | file | lines | changes | reviewer fixes | score |", "|---|---|---|---|---|---|"]
    for i, row in enumerate(rows, 1):
        out.append(f"| {i} | {row.path} | {row.lines} | {row.changes} | {row.fixes} | {row.score} |")
    return "\n".join(out)


def line_counter(root: str) -> Callable[[str], int | None]:
    def count(path: str) -> int | None:
        full = os.path.join(root, path)
        if not os.path.isfile(full) or os.path.islink(full):
            return None
        with open(full, "rb") as fh:
            return sum(1 for _ in fh)

    return count


class Mcp:
    """A minimal MCP stdio client of `fld mcp`."""

    def __init__(self) -> None:
        self.proc = subprocess.Popen(["fld", "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        self.next_id = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hotspots", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, msg: dict) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def request(self, method: str, params: dict) -> dict:
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            msg = json.loads(line)
            if msg.get("id") == self.next_id:
                if "error" in msg:
                    raise RuntimeError(f"{method}: {msg['error'].get('message')}")
                return msg["result"]
        raise RuntimeError("fld mcp closed the connection")

    def call(self, tool: str, args: dict[str, object]) -> dict:
        result = self.request("tools/call", {"name": tool, "arguments": args})
        text = result["content"][0]["text"]
        if result.get("isError"):
            raise RuntimeError(f"{tool}: {text}")
        return json.loads(text)

    def close(self) -> None:
        if self.proc.stdin:
            self.proc.stdin.close()
        self.proc.wait(timeout=10)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--revisions", type=int, default=50, help="how many of the newest revisions of main to read (default 50)")
    ap.add_argument("--top", type=int, default=20, help="how many rows to print (default 20)")
    ap.add_argument("--root", default=".", help="the folder whose files give the sizes (default the current folder)")
    ap.add_argument("--fixture", help="a JSON file of recorded tool results, keyed by tool name and arguments; no daemon is used")
    ap.add_argument("--show-excluded", action="store_true", help="also list the changed paths left out, with the reason")
    a = ap.parse_args(argv)

    if a.fixture:
        with open(a.fixture) as fh:
            recorded = json.load(fh)

        def call(tool: str, args: dict[str, object]) -> dict:
            return recorded[f"{tool} {json.dumps(args, sort_keys=True)}"]

        changes, fixes, seen = collect(call, a.revisions)
    else:
        client = Mcp()
        try:
            changes, fixes, seen = collect(client.call, a.revisions)
        finally:
            client.close()

    rows, left_out = rank(changes, fixes, line_counter(a.root))
    if seen:
        print(f"Revisions {seen[-1]}..{seen[0]} ({len(seen)}); score = changes x lines.\n")
    print(table(rows[: a.top]))
    if a.show_excluded:
        print("\nLeft out:")
        for path in sorted(left_out):
            print(f"- {path}: {left_out[path]}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
