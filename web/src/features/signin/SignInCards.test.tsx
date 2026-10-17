// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NAME_HINT } from "./name";
import { NameCard, TokenCard } from "./SignInCards";

afterEach(cleanup);

describe("NameCard", () => {
  it("asks what fold should call the human", () => {
    render(<NameCard pending={false} onSubmit={vi.fn()} />);
    expect(
      screen.getByRole("heading", { name: "What should fold call you?" }),
    ).toBeTruthy();
  });

  it("states the name rule", () => {
    render(<NameCard pending={false} onSubmit={vi.fn()} />);
    expect(screen.getByText(NAME_HINT)).toBeTruthy();
  });

  it("rejects an empty name and sends nothing", async () => {
    const onSubmit = vi.fn();
    render(<NameCard pending={false} onSubmit={onSubmit} />);
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByText("Enter a name.")).toBeTruthy();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("rejects a name over 32 characters", async () => {
    const onSubmit = vi.fn();
    render(<NameCard pending={false} onSubmit={onSubmit} />);
    await userEvent.type(screen.getByLabelText("Name"), "a".repeat(33));
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByText(/at most 32/)).toBeTruthy();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("rejects a name the daemon would refuse", async () => {
    const onSubmit = vi.fn();
    render(<NameCard pending={false} onSubmit={onSubmit} />);
    await userEvent.type(screen.getByLabelText("Name"), "Tanim");
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByText(/^Use only lowercase letters/)).toBeTruthy();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("sends the trimmed name", async () => {
    const onSubmit = vi.fn();
    render(<NameCard pending={false} onSubmit={onSubmit} />);
    await userEvent.type(screen.getByLabelText("Name"), "  tanim ");
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onSubmit).toHaveBeenCalledExactlyOnceWith("tanim");
  });

  it("is busy while the write is in flight", () => {
    render(<NameCard pending onSubmit={vi.fn()} />);
    const button = screen.getByRole("button", { name: "Continuing…" });
    expect(button.getAttribute("aria-busy")).toBe("true");
    expect((button as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows the daemon's refusal inline", () => {
    render(<NameCard pending={false} error="Refused." onSubmit={vi.fn()} />);
    expect(screen.getByText("Refused.")).toBeTruthy();
  });
});

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
