// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Button, IconButton } from "./Button";

afterEach(cleanup);

describe("Button", () => {
  it("is a named button that is not busy by default", () => {
    render(<Button>Save</Button>);
    const button = screen.getByRole("button", { name: "Save" });
    expect(button).toHaveProperty("disabled", false);
    expect(button.getAttribute("aria-busy")).toBeNull();
    expect(button.getAttribute("type")).toBe("button");
  });

  it("says its pending label, is disabled and busy while pending, and does not fire", async () => {
    const onClick = vi.fn();
    render(
      <Button pending pendingLabel="Saving…" onClick={onClick}>
        Save
      </Button>,
    );
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    const button = screen.getByRole("button", { name: "Saving…" });
    expect(button).toHaveProperty("disabled", true);
    expect(button.getAttribute("aria-busy")).toBe("true");
    await userEvent.click(button);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("says its idle label when a write control is not pending", () => {
    render(
      <Button pending={false} pendingLabel="Saving…">
        Save
      </Button>,
    );
    expect(screen.getByRole("button", { name: "Save" })).toHaveProperty(
      "disabled",
      false,
    );
  });

  it("is disabled when disabled", () => {
    render(<Button disabled>Save</Button>);
    expect(screen.getByRole("button", { name: "Save" })).toHaveProperty(
      "disabled",
      true,
    );
  });

  it("fires on click and on the keyboard", async () => {
    const onClick = vi.fn();
    render(<Button onClick={onClick}>Save</Button>);
    await userEvent.tab();
    expect(document.activeElement).toBe(screen.getByRole("button"));
    await userEvent.keyboard("{Enter}");
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("shows the icon only when not pending", () => {
    const { rerender } = render(
      <Button icon={<i data-testid="icon" />}>Save</Button>,
    );
    expect(screen.queryByTestId("icon")).not.toBeNull();
    rerender(
      <Button pending pendingLabel="Saving…" icon={<i data-testid="icon" />}>
        Save
      </Button>,
    );
    expect(screen.queryByTestId("icon")).toBeNull();
  });
});

describe("IconButton", () => {
  it("is named by its aria-label", () => {
    render(
      <IconButton aria-label="Close">
        <i />
      </IconButton>,
    );
    expect(screen.getByRole("button", { name: "Close" })).toHaveProperty(
      "disabled",
      false,
    );
  });

  it("is named by its pending label, disabled and busy while pending", () => {
    render(
      <IconButton aria-label="Close" pending pendingLabel="Closing…">
        <i />
      </IconButton>,
    );
    const button = screen.getByRole("button", { name: "Closing…" });
    expect(button).toHaveProperty("disabled", true);
    expect(button.getAttribute("aria-busy")).toBe("true");
  });

  it("fails type checking for a pending control without a pending label", () => {
    // @ts-expect-error a pending control needs its pending label
    const element = <Button pending>x</Button>;
    expect(element).toBeTruthy();
  });

  it("fails type checking without a label", () => {
    // @ts-expect-error an icon button needs an aria-label
    const element = <IconButton>x</IconButton>;
    expect(element).toBeTruthy();
  });
});
