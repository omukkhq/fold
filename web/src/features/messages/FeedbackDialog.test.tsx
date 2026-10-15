// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "@/api/errors";
import { FeedbackDialog } from "./FeedbackDialog";

afterEach(cleanup);

function setup(onSend: (task: string, body: string) => Promise<void>) {
  const onOpenChange = vi.fn();
  render(<FeedbackDialog open onOpenChange={onOpenChange} onSend={onSend} />);
  return onOpenChange;
}

describe("FeedbackDialog", () => {
  it("validates before it sends", async () => {
    const onSend = vi.fn(() => Promise.resolve());
    setup(onSend);
    await userEvent.click(
      screen.getByRole("button", { name: "Send feedback" }),
    );
    expect(
      await screen.findByText("Enter a task id such as T-12."),
    ).toBeTruthy();
    expect(screen.getByText("Write a message first.")).toBeTruthy();
    expect(onSend).not.toHaveBeenCalled();
  });

  it("shows pending while it sends, then closes", async () => {
    let finish: () => void = () => {};
    const onSend = vi.fn(
      () => new Promise<void>((resolve) => (finish = resolve)),
    );
    const onOpenChange = setup(onSend);
    await userEvent.type(screen.getByLabelText("Task"), "T-12");
    await userEvent.type(screen.getByLabelText("Message"), "Shorter please");
    await userEvent.click(
      screen.getByRole("button", { name: "Send feedback" }),
    );
    await waitFor(() => {
      expect(
        screen
          .getByRole("button", { name: "Send feedback" })
          .getAttribute("aria-busy"),
      ).toBe("true");
    });
    expect(onSend).toHaveBeenCalledWith("T-12", "Shorter please");
    expect(onOpenChange).not.toHaveBeenCalled();
    finish();
    await waitFor(() => {
      expect(onOpenChange).toHaveBeenCalledWith(false);
    });
  });

  it("words the refusal and keeps the text when the daemon refuses", async () => {
    setup(() => Promise.reject(new ApiError("task_not_found", "no")));
    await userEvent.type(screen.getByLabelText("Task"), "T-99");
    await userEvent.type(screen.getByLabelText("Message"), "Hi");
    await userEvent.click(
      screen.getByRole("button", { name: "Send feedback" }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "The message was not sent. No task has this id.",
    );
    expect(screen.getByLabelText<HTMLTextAreaElement>("Message").value).toBe(
      "Hi",
    );
  });
});
