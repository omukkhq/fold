import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import { getSession, setName, signIn } from "./session";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("getSession", () => {
  it("reads the session status", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
    });
    await expect(getSession()).resolves.toEqual({
      human: "tanim",
      authenticated: true,
    });
  });
});

describe("setName", () => {
  it("posts the name to the session route", async () => {
    const calls = stubFetch({ "POST /api/v1/session": { status: 204 } });
    await setName("tanim");
    expect(calls).toHaveLength(1);
    expect(calls[0]?.path).toBe("/api/v1/session");
    expect(calls[0]?.body).toEqual({ name: "tanim" });
  });
});

describe("signIn", () => {
  it("posts the token and nothing else", async () => {
    const calls = stubFetch({ "POST /api/v1/session": { status: 204 } });
    await signIn("fold_tk_abc");
    expect(calls[0]?.body).toEqual({ token: "fold_tk_abc" });
  });

  it("throws the daemon's code for a refused token", async () => {
    stubFetch({
      "POST /api/v1/session": errorReply(401, "invalid_token"),
    });
    await expect(signIn("bad")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "invalid_token"),
    );
  });

  it("throws a plain error when the body is not the error envelope", async () => {
    stubFetch({ "POST /api/v1/session": { status: 502, body: "bad gateway" } });
    await expect(signIn("x")).rejects.toThrow("the daemon answered 502");
  });
});
