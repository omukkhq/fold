// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "@/api/errors";
import { IntegratorDialog } from "./IntegratorDialog";

afterEach(cleanup);

function setup(onConfirm: () => Promise<void>, grant = true) {
  const onOpenChange = vi.fn();
  render(
    <IntegratorDialog
      agent="frontend"
      grant={grant}
      open
      onOpenChange={onOpenChange}
      onConfirm={onConfirm}
    />,
  );
  return onOpenChange;
}

describe("IntegratorDialog", () => {
  it("asks before it grants and calls nothing until confirmed", () => {
    const onConfirm = vi.fn(() => Promise.resolve());
    setup(onConfirm);
    expect(
      screen.getByRole("dialog", { name: "Grant integrator to frontend?" }),
    ).toBeTruthy();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("shows pending on Confirm, then closes when the daemon answers", async () => {
    let finish: () => void = () => {};
    const onConfirm = vi.fn(
      () => new Promise<void>((resolve) => (finish = resolve)),
    );
    const onOpenChange = setup(onConfirm);
    await userEvent.click(
      screen.getByRole("button", { name: "Grant integrator" }),
    );
    const busy = screen.getByRole("button", { name: "Grant integrator" });
    expect(busy.getAttribute("aria-busy")).toBe("true");
    expect((busy as HTMLButtonElement).disabled).toBe(true);
    expect(onOpenChange).not.toHaveBeenCalled();
    finish();
    await waitFor(() => {
      expect(onOpenChange).toHaveBeenCalledWith(false);
    });
  });

  it("names the removal and keeps the dialog open on an error code", async () => {
    const onConfirm = vi.fn(() =>
      Promise.reject(new ApiError("agent_not_found", "no")),
    );
    const onOpenChange = setup(onConfirm, false);
    await userEvent.click(
      screen.getByRole("button", { name: "Remove integrator" }),
    );
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(
      "The tag did not change. fold has no agent with this name.",
    );
    expect(onOpenChange).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("says the request did not arrive for an error without a code", async () => {
    setup(() => Promise.reject(new Error("boom")));
    await userEvent.click(
      screen.getByRole("button", { name: "Grant integrator" }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "The tag did not change. The request did not reach the daemon. Try again.",
    );
  });
});
