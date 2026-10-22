import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { keys } from "@/api/keys";
import { getSession, signIn, signInWithLink } from "@/api/session";
import { signInView, withoutFragment } from "./link";
import { signInErrorMessage } from "./messages";
import { safeRedirect } from "./redirect";
import { LinkCard, TokenCard } from "./SignInCards";

interface SignInScreenProps {
  /** The secret of the sign-in link the address carried, if any. */
  link: string | null;
  /** The path to return to, as it arrived in the address; it is checked before use. */
  redirect: string | undefined;
}

type Credential = { kind: "token" | "link"; value: string };

export function SignInScreen({ link, redirect }: SignInScreenProps) {
  const router = useRouter();
  const queryClient = useQueryClient();
  // Kept from the first render: the link leaves the address as soon as it is sent.
  const [secret] = useState(link);
  const [choseToken, setChoseToken] = useState(false);
  const submit = useMutation({
    mutationFn: async ({ kind, value }: Credential) => {
      await (kind === "token" ? signIn(value) : signInWithLink(value));
      return getSession();
    },
    onSuccess: (session) => {
      queryClient.setQueryData(keys.session(), session);
      void router.navigate({ href: safeRedirect(redirect), replace: true });
    },
  });
  const view = signInView(secret, choseToken);
  const sent = useRef(false);
  const { mutate } = submit;
  useEffect(() => {
    if (view.kind !== "link" || sent.current) {
      return;
    }
    sent.current = true;
    window.history.replaceState(
      window.history.state,
      "",
      withoutFragment(window.location.href),
    );
    mutate({ kind: "link", value: view.secret });
  }, [view, mutate]);
  const error = submit.isError ? signInErrorMessage(submit.error) : undefined;
  return (
    <main className="flex min-h-dvh items-center justify-center bg-bg px-24">
      {view.kind === "link" ? (
        <LinkCard
          error={error}
          onUseToken={() => {
            submit.reset();
            setChoseToken(true);
          }}
        />
      ) : (
        <TokenCard
          pending={submit.isPending}
          error={error}
          onSubmit={(value) => {
            submit.mutate({ kind: "token", value });
          }}
        />
      )}
    </main>
  );
}
