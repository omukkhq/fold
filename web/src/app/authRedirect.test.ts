import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { keys } from "../api/keys";
import { redirectToSignIn } from "./authRedirect";

describe("redirectToSignIn", () => {
  it("drops the cached session and returns to the place afterwards", () => {
    const client = new QueryClient();
    client.setQueryData(keys.session(), { human: null, authenticated: true });
    const navigate = vi.fn();
    redirectToSignIn(
      client,
      { pathname: "/p/demo/board", href: "/p/demo/board?agent=a" },
      navigate,
    );
    expect(client.getQueryData(keys.session())).toBeUndefined();
    expect(navigate).toHaveBeenCalledWith(
      "/sign-in?redirect=%2Fp%2Fdemo%2Fboard%3Fagent%3Da",
    );
  });

  it("does nothing on the sign-in screen", () => {
    const client = new QueryClient();
    client.setQueryData(keys.session(), { human: null, authenticated: false });
    const navigate = vi.fn();
    redirectToSignIn(
      client,
      { pathname: "/sign-in", href: "/sign-in?redirect=%2Fprojects" },
      navigate,
    );
    expect(navigate).not.toHaveBeenCalled();
    expect(client.getQueryData(keys.session())).toBeDefined();
  });
});
