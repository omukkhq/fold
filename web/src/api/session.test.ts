import { describe, expect, it } from "vitest";
import { postJson } from "./client";
import { isApiError } from "./errors";
import { setName, signIn } from "./session";

function recorder(status: number, body: unknown = {}) {
  const calls: { url: string; init: RequestInit | undefined }[] = [];
  const fetchImpl: typeof fetch = (input, init) => {
    calls.push({ url: typeof input === "string" ? input : "non-string", init });
    return Promise.resolve(new Response(JSON.stringify(body), { status }));
  };
  return { calls, fetchImpl };
}

describe("setName", () => {
  it("posts the name to the session route", async () => {
    const r = recorder(200);
    await setName("tanim", r.fetchImpl);
    expect(r.calls).toHaveLength(1);
    expect(r.calls[0]?.url).toBe("/api/v1/session");
    expect(r.calls[0]?.init?.method).toBe("POST");
    expect(r.calls[0]?.init?.body).toBe(JSON.stringify({ name: "tanim" }));
  });
});

describe("signIn", () => {
  it("posts the token and nothing else", async () => {
    const r = recorder(200);
    await signIn("fold_tk_abc", r.fetchImpl);
    expect(r.calls[0]?.init?.body).toBe(
      JSON.stringify({ token: "fold_tk_abc" }),
    );
  });

  it("throws the daemon's code for a refused token", async () => {
    const r = recorder(401, {
      error: { code: "invalid_token", message: "no", details: {} },
    });
    await expect(signIn("bad", r.fetchImpl)).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "invalid_token"),
    );
  });
});

describe("postJson", () => {
  it("throws a plain error when the body is not the error envelope", async () => {
    const r = recorder(502, "bad gateway");
    await expect(postJson("/x", {}, r.fetchImpl)).rejects.toThrow(
      "the daemon answered 502",
    );
  });
});
