import { describe, expect, it } from "vitest";
import { listReviewCheckouts } from "./reviews";

describe("listReviewCheckouts", () => {
  it("lists the fixture checkout of the demo project", async () => {
    const rows = await listReviewCheckouts("demo");
    expect(rows[0]).toMatchObject({ proposal: "P-4", version: 2 });
  });

  it("is empty for another project", async () => {
    expect(await listReviewCheckouts("other")).toEqual([]);
  });
});
