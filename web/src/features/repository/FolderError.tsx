import { CodeText } from "@/components/code/CodeText";
import { WriteError } from "@/features/task/parts";
import { folderErrorText } from "./errors";

/** A refused write to the project folder or its stash: the sentence for its reason, or the generic banner with the daemon's message. */
export function FolderError({ error }: { error: Error | null }) {
  if (error === null) {
    return null;
  }
  const text = folderErrorText(error);
  return text === null ? (
    <WriteError error={error} />
  ) : (
    <p
      role="alert"
      className="max-w-measure text-13 font-medium wrap-anywhere text-bad"
    >
      <CodeText text={text} caption />
    </p>
  );
}
