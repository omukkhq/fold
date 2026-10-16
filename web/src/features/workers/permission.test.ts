import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { permissionQuestion } from "@/test/workers";
import {
  answerFailure,
  answerLine,
  asksText,
  inputView,
  whyAsksText,
  shownQuestion,
  toolIcon,
} from "./permission";

describe("inputView", () => {
  it("shows a command with its description, a path, a URL, or else JSON", () => {
    expect(
      inputView({ command: "bun add zod", description: "in web/" }),
    ).toEqual({
      text: "bun add zod",
      note: "in web/",
    });
    expect(inputView({ command: "make deploy" })).toEqual({
      text: "make deploy",
      note: null,
    });
    expect(inputView({ file_path: "web/src/a.ts", content: "x" }).text).toBe(
      "web/src/a.ts",
    );
    expect(inputView({ path: "web" }).text).toBe("web");
    expect(inputView({ url: "ftp.example.test/page" }).text).toBe(
      "ftp.example.test/page",
    );
    expect(inputView({ query: 3 }).text).toBe('{\n  "query": 3\n}');
  });
});

describe("tools", () => {
  it("picks an icon and words the request per tool", () => {
    expect(toolIcon("Bash")).toBe("terminal");
    expect(toolIcon("Write")).toBe("file");
    expect(toolIcon("WebFetch")).toBe("globe");
    expect(toolIcon("mcp__x__y")).toBe("wrench");
    expect(asksText("Bash")).toBe("asks to run Bash");
    expect(asksText("Write")).toBe("asks to use Write");
  });
});

describe("answers", () => {
  const answer = {
    behavior: "allow" as const,
    answered_by: "human:tanim",
    message: null,
    answered_at: 1,
  };

  it("words an allowed and a denied call", () => {
    expect(answerLine(answer)).toEqual({
      allowed: true,
      lead: "Allowed by ",
      by: "human:tanim",
      after: "the worker went on",
    });
    expect(answerLine({ ...answer, behavior: "deny" }).after).toBe(
      "the worker went on without the call",
    );
  });

  it("prefers this session's answered question over the open list", () => {
    const open = permissionQuestion();
    const answered = permissionQuestion({ answer });
    expect(shownQuestion([open], answered, "pq-3")).toBe(answered);
    expect(shownQuestion([open], null, "pq-3")).toBe(open);
    expect(shownQuestion([open], answered, "pq-4")).toBeNull();
  });

  it("words a refused answer", () => {
    expect(
      answerFailure(new ApiError("permission_question_closed", "")).text,
    ).toContain("closed");
    expect(
      answerFailure(new ApiError("permission_question_not_found", "")).text,
    ).toContain("no permission question");
    expect(answerFailure(new ApiError("human_name_required", "")).code).toBe(
      "human_name_required",
    );
  });
});

describe("whyAsksText", () => {
  it("names the tool and that the permission rules asked, not the input", () => {
    expect(whyAsksText("Bash")).toBe(
      "Claude Code's permission rules asked for a human before the worker used Bash. The call runs only if you allow it.",
    );
  });
});
