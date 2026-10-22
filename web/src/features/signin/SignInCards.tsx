import { ArrowRight } from "lucide-react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import type { ReactNode } from "react";
import { Button } from "@/components/button/Button";
import { TextField } from "@/components/form/controls";
import { Mono } from "@/components/mono/Mono";
import { Wordmark } from "@/components/Wordmark";
import { tokenFormSchema, type TokenFormValues } from "./token";

function Card({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <section
      aria-label={title}
      className="flex w-sign-in max-w-full flex-col gap-16 rounded-l bg-surface p-32 inset-ring inset-ring-border-subtle"
    >
      <Wordmark />
      <div className="flex flex-col gap-4">
        <h1 className="text-screen leading-snug font-semibold text-text">
          {title}
        </h1>
        <p className="text-body text-text-muted">{description}</p>
      </div>
      {children}
    </section>
  );
}

interface CardProps {
  pending: boolean;
  /** The failure of the last attempt, already worded from its code. */
  error?: string;
  onSubmit: (value: string) => void;
}

export function TokenCard({ pending, error, onSubmit }: CardProps) {
  const form = useForm<TokenFormValues>({
    resolver: zodResolver(tokenFormSchema),
    defaultValues: { token: "" },
  });
  return (
    <Card
      title="Sign in"
      description="Open the sign-in link that fld web prints on the machine that runs fold, or enter a token that fld token create made."
    >
      <form
        noValidate
        className="flex flex-col gap-16"
        onSubmit={(event) => {
          void form.handleSubmit((values) => {
            onSubmit(values.token);
          })(event);
        }}
      >
        <TextField
          label="Token"
          type="password"
          placeholder="43 characters from fld token create"
          error={form.formState.errors.token?.message ?? error}
          className="font-mono text-meta"
          autoComplete="off"
          spellCheck={false}
          autoFocus
          {...form.register("token")}
        />
        <Button
          type="submit"
          variant="primary"
          className="w-full"
          pending={pending}
          pendingLabel="Signing in…"
          icon={<ArrowRight aria-hidden size={15} />}
        >
          Sign in
        </Button>
      </form>
      <Mono className="text-meta text-text-muted">
        fld web · fld token create &lt;name&gt;
      </Mono>
    </Card>
  );
}

interface LinkCardProps {
  /** The failure of the link, already worded from its code; while it is absent the sign-in is pending. */
  error?: string;
  onUseToken: () => void;
}

/** The sign-in a link from `fld web` opens: pending until the daemon answers, then its refusal and the way on. */
export function LinkCard({ error, onUseToken }: LinkCardProps) {
  if (error === undefined) {
    return (
      <Card title="Sign in" description="fold checks your sign-in link.">
        <Button
          variant="primary"
          className="w-full"
          pending
          pendingLabel="Signing in…"
        >
          Sign in
        </Button>
      </Card>
    );
  }
  return (
    <Card title="Sign in" description={error}>
      <Mono className="text-meta text-text-muted">fld web</Mono>
      <Button className="w-full" onClick={onUseToken}>
        Sign in with a token
      </Button>
    </Card>
  );
}
