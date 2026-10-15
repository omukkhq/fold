import { describe, expect, it } from "vitest";
import { olderNotListed, readPages } from "./pages";

describe("readPages", () => {
  const pages = new Map([
    [undefined, { items: [1, 2], next: "b" }],
    ["b", { items: [3], next: "c" }],
    ["c", { items: [4], next: null }],
  ]);
  const read = (cursor: string | undefined) => {
    const page = pages.get(cursor);
    return page ? Promise.resolve(page) : Promise.reject(new Error("no page"));
  };

  it("joins the pages up to the last", async () => {
    await expect(readPages(read, 10)).resolves.toEqual({
      items: [1, 2, 3, 4],
      more: false,
    });
  });

  it("stops after the page limit and says the list goes on", async () => {
    await expect(readPages(read, 2)).resolves.toEqual({
      items: [1, 2, 3],
      more: true,
    });
  });

  it("says the list ends when the last page is the limit", async () => {
    await expect(readPages(read, 3)).resolves.toEqual({
      items: [1, 2, 3, 4],
      more: false,
    });
  });
});

describe("olderNotListed", () => {
  it("names the count and the items left out", () => {
    expect(olderNotListed(1000, "threads")).toBe(
      "Showing the newest 1000 threads. Older threads are not listed.",
    );
  });
});
