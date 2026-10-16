import { useRouter } from "@tanstack/react-router";
import { ArrowRight, GitCompare } from "lucide-react";
import { useId, useState } from "react";
import type { Revision } from "@/api/repository";
import { Button } from "@/components/button/Button";
import { controlClasses } from "@/components/form/Field";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  compareLabel,
  comparePath,
  defaultCompareFrom,
  normalizeCompare,
} from "./compare";

interface CompareBarProps {
  slug: string;
  /** Newest first, with at least two revisions. */
  revisions: Revision[];
  /** The revision the tree shows; it is the default end of the comparison. */
  selected: string;
}

const REASONS = {
  same: "Choose two different revisions.",
  invalid: "Choose two revisions.",
} as const;

// The comparison is one button in the tree's bar, so the two choices sit inline at the button's height.
const SELECT = cn(controlClasses, focusRing, "w-auto py-7 font-mono text-bar");

export function CompareBar({ slug, revisions, selected }: CompareBarProps) {
  const router = useRouter();
  const errorId = useId();
  const [from, setFrom] = useState(
    () => defaultCompareFrom(revisions, selected) ?? selected,
  );
  const [to, setTo] = useState(selected);
  const [error, setError] = useState<string | undefined>();

  function compare() {
    const result = normalizeCompare(from, to);
    if (!result.ok) {
      setError(REASONS[result.reason]);
      return;
    }
    setError(undefined);
    void router.navigate({ href: comparePath(slug, result.pair) });
  }

  const options = revisions.map((r) => (
    <option key={r.id} value={r.id}>
      {r.id}
    </option>
  ));
  const control = {
    "aria-invalid": error === undefined ? undefined : true,
    "aria-describedby": error === undefined ? undefined : errorId,
    className: SELECT,
  };
  return (
    <form
      aria-label="Compare revisions"
      className="flex flex-wrap items-center justify-end gap-10"
      onSubmit={(event) => {
        event.preventDefault();
        compare();
      }}
    >
      {error === undefined ? null : (
        <p id={errorId} className="text-meta font-medium text-danger">
          {error}
        </p>
      )}
      <select
        {...control}
        aria-label="From"
        value={from}
        onChange={(event) => {
          setFrom(event.target.value);
        }}
      >
        {options}
      </select>
      <ArrowRight aria-hidden size={13} className="text-text-faint" />
      <select
        {...control}
        aria-label="To"
        value={to}
        onChange={(event) => {
          setTo(event.target.value);
        }}
      >
        {options}
      </select>
      <Button type="submit" icon={<GitCompare aria-hidden size={15} />}>
        {compareLabel(from, to)}
      </Button>
    </form>
  );
}
