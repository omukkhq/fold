import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect } from "vitest";

/** Types `#T-` and `@w` into a mention box and checks that the popover offers the item and writes each pick. */
export async function expectMentionPopover(box: HTMLElement): Promise<void> {
  await userEvent.type(box, "see #T-");
  await userEvent.click(await screen.findByRole("option", { name: /#T-9/ }));
  expect(box).toHaveProperty("value", "see #T-9 ");
  await userEvent.type(box, "in @w");
  await userEvent.click(await screen.findByRole("option", { name: /@web\// }));
  expect(box).toHaveProperty("value", "see #T-9 in @web/");
  await screen.findByRole("option", { name: /@report\.tsx/ });
  await userEvent.keyboard("{Escape}");
  await waitFor(() => {
    expect(screen.queryByRole("listbox")).toBeNull();
  });
}
