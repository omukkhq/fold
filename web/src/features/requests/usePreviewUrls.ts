import { useEffect, useRef, useState } from "react";
import { PreviewUrls } from "./preview";

/** The object URL of each previewable file in the list; the URLs are revoked when a file leaves the list and when the component unmounts. */
export function usePreviewUrls(
  files: readonly File[],
): ReadonlyMap<File, string> {
  const registry = useRef<PreviewUrls | null>(null);
  const [urls, setUrls] = useState<ReadonlyMap<File, string>>(new Map());
  useEffect(() => {
    registry.current ??= new PreviewUrls();
    setUrls(registry.current.sync(files));
  }, [files]);
  useEffect(
    () => () => {
      registry.current?.releaseAll();
      registry.current = null;
    },
    [],
  );
  return urls;
}
