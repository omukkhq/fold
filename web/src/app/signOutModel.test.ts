import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { keys } from "../api/keys";
import { initials, leaveAfterSignOut } from "./signOutModel";

describe("leaveAfterSignOut", () => {
  it("drops the session before it opens the sign-in screen, then the rest of the cache", async () => {
    const client = new QueryClient();
    client.setQueryData(keys.session(), {
      human: "tanim",
      authenticated: true,
    });
    client.setQueryData(["projects"], []);
    const seen: { href: string; session: unknown; projects: unknown }[] = [];
    await leaveAfterSignOut(client, (href) => {
      seen.push({
        href,
        session: client.getQueryData(keys.session()),
        projects: client.getQueryData(["projects"]),
      });
      return Promise.resolve();
    });
    expect(seen).toEqual([
      { href: "/sign-in", session: undefined, projects: [] },
    ]);
    expect(client.getQueryData(["projects"])).toBeUndefined();
  });
});

describe("initials", () => {
  it("takes the first letters of two parts, or the first two letters of one", () => {
    expect(initials("tanim-a")).toBe("TA");
    expect(initials("pptx704")).toBe("PP");
    expect(initials("x")).toBe("X");
  });

  it("has none without a name or without letters", () => {
    expect(initials(null)).toBeNull();
    expect(initials("--")).toBeNull();
  });
});
