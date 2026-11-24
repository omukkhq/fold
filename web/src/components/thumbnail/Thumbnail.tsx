import { cn } from "@/lib/cn";

interface ThumbnailProps {
  src: string;
  /** The file name, which is the image's text for a screen reader. */
  name: string;
  className?: string;
}

/** An image in a fixed box from the tokens; it keeps its aspect ratio and is never cropped. */
export function Thumbnail({ src, name, className }: ThumbnailProps) {
  return (
    <span
      className={cn(
        "flex h-thumb w-thumb shrink-0 items-center justify-center overflow-hidden rounded-m bg-sunken",
        className,
      )}
    >
      <img
        src={src}
        alt={name}
        className="max-h-full max-w-full object-contain"
      />
    </span>
  );
}
