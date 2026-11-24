/** Media types a browser shows as an image without scripting; an SVG is left out because it can hold script. */
const PREVIEW_TYPES: ReadonlySet<string> = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
]);

/** A picker leaves `File.type` empty for a file the system has no type for, so the extension stands in. */
const EXTENSION_TYPES: Readonly<Record<string, string>> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
};

/** The media type without its parameters, in lower case. */
function baseType(mediaType: string): string {
  return (mediaType.split(";")[0] ?? "").trim().toLowerCase();
}

/** Whether a stored media type previews as an image. */
export function isPreviewable(mediaType: string): boolean {
  return PREVIEW_TYPES.has(baseType(mediaType));
}

/** Whether a picked file previews, by its type or, when the type is empty, by its extension. */
export function isPreviewableFile(file: {
  name: string;
  type: string;
}): boolean {
  if (baseType(file.type) !== "") return isPreviewable(file.type);
  const dot = file.name.lastIndexOf(".");
  if (dot < 0) return false;
  const known = EXTENSION_TYPES[file.name.slice(dot + 1).toLowerCase()];
  return known !== undefined;
}

export interface UrlApi {
  create: (file: File) => string;
  revoke: (url: string) => void;
}

const browserUrls: UrlApi = {
  create: (file) => URL.createObjectURL(file),
  revoke: (url) => {
    URL.revokeObjectURL(url);
  },
};

/**
 * The object URLs of the picked images. A URL lives from the time its file is picked until the file leaves
 * the list or `releaseAll` runs, so a removed file or a closed form never leaves bytes pinned in memory.
 */
export class PreviewUrls {
  private readonly urls = new Map<File, string>();

  constructor(private readonly api: UrlApi = browserUrls) {}

  /** Makes a URL for each new previewable file and revokes the URL of each file that is gone. */
  sync(files: readonly File[]): ReadonlyMap<File, string> {
    const keep = new Set(files);
    for (const [file, url] of this.urls) {
      if (!keep.has(file)) {
        this.api.revoke(url);
        this.urls.delete(file);
      }
    }
    for (const file of files) {
      if (!this.urls.has(file) && isPreviewableFile(file)) {
        this.urls.set(file, this.api.create(file));
      }
    }
    return new Map(this.urls);
  }

  releaseAll(): void {
    for (const url of this.urls.values()) this.api.revoke(url);
    this.urls.clear();
  }
}
