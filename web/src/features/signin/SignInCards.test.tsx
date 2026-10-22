// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LinkCard, TokenCard } from "./SignInCards";

afterEach(cleanup);

describe("TokenCard", () => {
  it("hides the token while it is typed and sends it trimmed", async () => {
    const onSubmit = vi.fn();
    render(<TokenCard pending={false} onSubmit={onSubmit} />);
    const field = screen.getByLabelText("Token");
    expect(field.getAttribute("type")).toBe("password");
    await userEvent.type(field, " fold_tk_abc ");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(onSubmit).toHaveBeenCalledExactlyOnceWith("fold_tk_abc");
  });

  it("shows a pending state and an inline error", () => {
    const { rerender } = render(<TokenCard pending onSubmit={vi.fn()} />);
    expect(
      screen
        .getByRole("button", { name: "Signing in…" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    rerender(
      <TokenCard pending={false} error="Not accepted." onSubmit={vi.fn()} />,
    );
    expect(screen.getByText("Not accepted.")).toBeTruthy();
  });
});

describe("LinkCard", () => {
  it("is pending while the daemon checks the link", () => {
    render(<LinkCard onUseToken={vi.fn()} />);
    const button = screen.getByRole("button", { name: "Signing in…" });
    expect(button.getAttribute("aria-busy")).toBe("true");
    expect((button as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows the refusal and offers the token form", async () => {
    const onUseToken = vi.fn();
    render(<LinkCard error="That link is used." onUseToken={onUseToken} />);
    expect(screen.getByText("That link is used.")).toBeTruthy();
    expect(screen.getByText("fld web")).toBeTruthy();
    await userEvent.click(
      screen.getByRole("button", { name: "Sign in with a token" }),
    );
    expect(onUseToken).toHaveBeenCalledOnce();
  });
});
