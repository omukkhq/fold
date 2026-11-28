#!/usr/bin/env python3
"""Tests of scripts/hotspots.py; run with `python3 -I scripts/hotspots_test.py`."""

import contextlib
import io
import json
import os
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import hotspots  # noqa: E402

FILES = {
    "crates/fold-daemon/src/live.rs": 300,
    "crates/fold-core/src/dag.rs": 100,
    "web/src/board.ts": 50,
    "web/src/api/schema.d.ts": 9000,
    "web/src/board.test.ts": 800,
    "crates/fold-daemon/tests/main.rs": 700,
    "crates/fold-cli/src/doctor/tests.rs": 600,
    "Cargo.lock": 5000,
    ".sqlx/query-1.json": 40,
}


def recorded() -> dict:
    def key(tool: str, args: dict) -> str:
        return f"{tool} {json.dumps(args, sort_keys=True)}"

    def rev(r: str, parent: str | None, proposal: str, paths: list[str]) -> tuple[str, dict]:
        return key("get_revision", {"r": r}), {"r": r, "parent": parent, "proposal": proposal, "files": [{"path": p} for p in paths]}

    every = list(FILES) + ["crates/fold-core/src/gone.rs"]
    data = dict(
        [
            (key("get_repository_state", {}), {"head": "r3"}),
            rev("r3", "r2", "P-2", every),
            rev("r2", "r1", "P-1", ["crates/fold-daemon/src/live.rs", "web/src/board.ts", "web/src/api/schema.d.ts"]),
            rev("r1", None, "P-0", ["crates/fold-daemon/src/live.rs"]),
        ]
    )
    data[key("get_proposal", {"id": "P-2"})] = {"versions": [{"changeset": "C-1"}, {"changeset": "C-2", "reviewer_fix": True}]}
    data[key("get_proposal", {"id": "P-1"})] = {"versions": [{"changeset": "C-3"}, {"changeset": "C-4"}]}
    data[key("get_proposal", {"id": "P-0"})] = {"versions": [{"changeset": "C-5"}]}
    data[key("compare_revisions", {"from": "C-1", "to": "C-2"})] = {"files": [{"path": "crates/fold-core/src/dag.rs"}, {"path": "web/src/board.test.ts"}]}
    return data


class Exclusion(unittest.TestCase):
    def test_tests_and_generated_files_are_left_out(self) -> None:
        for path in ["web/src/api/schema.d.ts", "Cargo.lock", "bun.lock", ".sqlx/q.json"]:
            self.assertEqual(hotspots.excluded(path), "generated", path)
        for path in ["crates/a/tests/main.rs", "crates/a/src/doctor/tests.rs", "web/src/x.test.ts", "web/src/x.spec.tsx", "scripts/hotspots_test.py"]:
            self.assertEqual(hotspots.excluded(path), "test", path)
        for path in ["crates/fold-daemon/src/live.rs", "web/src/board.ts", "scripts/build-gate.sh", "crates/fold-core/src/testing.rs"]:
            self.assertIsNone(hotspots.excluded(path), path)


class FixtureRun(unittest.TestCase):
    def test_ranked_table_from_recorded_tool_results(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            for path, lines in FILES.items():
                full = os.path.join(root, path)
                os.makedirs(os.path.dirname(full) or root, exist_ok=True)
                with open(full, "w") as fh:
                    fh.write("x\n" * lines)
            fixture = os.path.join(root, "fixture.json")
            with open(fixture, "w") as fh:
                json.dump(recorded(), fh)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                code = hotspots.main(["--fixture", fixture, "--root", root, "--show-excluded"])
        self.assertEqual(code, 0)
        text = out.getvalue()
        table = text.split("Left out:")[0]
        self.assertIn("Revisions r1..r3 (3)", text)
        self.assertIn("| 1 | crates/fold-daemon/src/live.rs | 300 | 3 | 0 | 900 |", table)
        self.assertIn("| 2 | crates/fold-core/src/dag.rs | 100 | 1 | 1 | 100 |", table)
        self.assertIn("| 3 | web/src/board.ts | 50 | 2 | 0 | 100 |", table)
        for path in ["schema.d.ts", "board.test.ts", "tests/main.rs", "doctor/tests.rs", "Cargo.lock", ".sqlx", "gone.rs"]:
            self.assertNotIn(path, table)
        self.assertIn("- web/src/api/schema.d.ts: generated", text)
        self.assertIn("- web/src/board.test.ts: test", text)
        self.assertIn("- crates/fold-core/src/gone.rs: absent", text)

    def test_revisions_limit_stops_the_walk(self) -> None:
        data = recorded()
        changes, _, seen = hotspots.collect(lambda tool, args: data[f"{tool} {json.dumps(args, sort_keys=True)}"], 2)
        self.assertEqual(seen, ["r3", "r2"])
        self.assertEqual(changes["crates/fold-daemon/src/live.rs"], 2)


if __name__ == "__main__":
    unittest.main()
