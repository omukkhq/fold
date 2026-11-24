import { describe, expect, it, vi } from "vitest";
import { PreviewUrls, isPreviewable, isPreviewableFile } from "./preview";

const file = (name: string, type: string) => new File(["x"], name, { type });

describe("isPreviewable", () => {
  it("accepts png, jpeg, gif and webp, whatever the case or parameters", () => {
    for (const type of [
      "image/png",
      "image/jpeg",
      "image/gif",
      "image/webp",
      "IMAGE/PNG",
      "image/png; charset=binary",
    ]) {
      expect(isPreviewable(type), type).toBe(true);
    }
  });

  it("refuses other types, an SVG too", () => {
    for (const type of ["image/svg+xml", "text/csv", "application/pdf", ""]) {
      expect(isPreviewable(type), type).toBe(false);
    }
  });
});

describe("isPreviewableFile", () => {
  it("follows the type of the file", () => {
    expect(isPreviewableFile(file("a.png", "image/png"))).toBe(true);
    expect(isPreviewableFile(file("a.png", "text/plain"))).toBe(false);
    expect(isPreviewableFile(file("a.csv", "text/csv"))).toBe(false);
  });

  it("falls back to the extension when the type is empty", () => {
    expect(isPreviewableFile(file("shot.JPG", ""))).toBe(true);
    expect(isPreviewableFile(file("shot.webp", ""))).toBe(true);
    expect(isPreviewableFile(file("shot.svg", ""))).toBe(false);
    expect(isPreviewableFile(file("shot", ""))).toBe(false);
  });
});

describe("PreviewUrls", () => {
  function setup() {
    let n = 0;
    const create = vi.fn(() => `blob:${String((n += 1))}`);
    const revoke = vi.fn();
    return { urls: new PreviewUrls({ create, revoke }), create, revoke };
  }

  it("makes one URL for each previewable file and none for the rest", () => {
    const { urls, create } = setup();
    const png = file("a.png", "image/png");
    const csv = file("a.csv", "text/csv");
    const map = urls.sync([png, csv]);
    expect(map.get(png)).toBe("blob:1");
    expect(map.has(csv)).toBe(false);
    urls.sync([png, csv]);
    expect(create).toHaveBeenCalledTimes(1);
  });

  it("revokes the URL of a file that leaves the list", () => {
    const { urls, revoke } = setup();
    const a = file("a.png", "image/png");
    const b = file("b.gif", "image/gif");
    urls.sync([a, b]);
    const map = urls.sync([b]);
    expect(revoke).toHaveBeenCalledTimes(1);
    expect(revoke).toHaveBeenCalledWith("blob:1");
    expect(map.get(b)).toBe("blob:2");
    expect(map.has(a)).toBe(false);
  });

  it("revokes every URL on release and starts clean after it", () => {
    const { urls, revoke } = setup();
    urls.sync([file("a.png", "image/png"), file("b.png", "image/png")]);
    urls.releaseAll();
    expect(revoke).toHaveBeenCalledTimes(2);
    urls.releaseAll();
    expect(revoke).toHaveBeenCalledTimes(2);
    expect(urls.sync([]).size).toBe(0);
  });

  it("uses the browser's object URLs by default", () => {
    const create = vi.fn(() => "blob:x");
    const revoke = vi.fn();
    Object.defineProperty(URL, "createObjectURL", {
      value: create,
      configurable: true,
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      value: revoke,
      configurable: true,
    });
    try {
      const urls = new PreviewUrls();
      urls.sync([file("a.png", "image/png")]);
      urls.releaseAll();
      expect(create).toHaveBeenCalledTimes(1);
      expect(revoke).toHaveBeenCalledWith("blob:x");
    } finally {
      Reflect.deleteProperty(URL, "createObjectURL");
      Reflect.deleteProperty(URL, "revokeObjectURL");
    }
  });
});
