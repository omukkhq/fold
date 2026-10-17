import { ArrowRight } from "lucide-react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import type { ReactNode } from "react";
import { Button } from "@/components/button/Button";
import { TextField } from "@/components/form/controls";
import { Mono } from "@/components/mono/Mono";
import { Wordmark } from "@/components/Wordmark";
import { NAME_HINT, nameFormSchema, type NameFormValues } from "./name";
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

export function NameCard({ pending, error, onSubmit }: CardProps) {
  const form = useForm<NameFormValues>({
    resolver: zodResolver(nameFormSchema),
    defaultValues: { name: "" },
  });
  return (
    <Card
      title="What should fold call you?"
      description="Your reviews, feedback and approvals carry this name. Asked once per browser on localhost."
    >
      <form
        noValidate
        className="flex flex-col gap-16"
        onSubmit={(event) => {
          void form.handleSubmit((values) => {
            onSubmit(values.name.trim());
          })(event);
        }}
      >
        <TextField
          label="Name"
          placeholder="e.g. tanim"
          error={form.formState.errors.name?.message ?? error}
          autoComplete="nickname"
          autoCapitalize="none"
          spellCheck={false}
          autoFocus
          {...form.register("name")}
        />
        <Button
          type="submit"
          variant="primary"
          className="w-full"
          pending={pending}
          pendingLabel="Continuing…"
          icon={<ArrowRight aria-hidden size={15} />}
        >
          Continue
        </Button>
      </form>
      <p className="text-meta text-text-muted">{NAME_HINT}</p>
    </Card>
  );
}

export function TokenCard({ pending, error, onSubmit }: CardProps) {
  const form = useForm<TokenFormValues>({
    resolver: zodResolver(tokenFormSchema),
    defaultValues: { token: "" },
  });
  return (
    <Card
      title="Sign in"
      description="This fold daemon listens on the network, so it needs a token. Ask whoever runs it, or create one on the host."
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
          placeholder="fold_tk_…"
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
        fld token create &lt;name&gt;
      </Mono>
    </Card>
  );
}
