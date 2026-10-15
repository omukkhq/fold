import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { keys } from "@/api/keys";
import { getSession, setName, signIn } from "@/api/session";
import { signInErrorMessage } from "./messages";
import type { SignInMode } from "./mode";
import { safeRedirect } from "./redirect";
import { NameCard, TokenCard } from "./SignInCards";

interface SignInScreenProps {
  /** `ready` is not shown: the route redirects away before it renders. */
  mode: Exclude<SignInMode, "ready">;
  /** The path to return to, as it arrived in the address; it is checked before use. */
  redirect: string | undefined;
}

export function SignInScreen({ mode, redirect }: SignInScreenProps) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const submit = useMutation({
    mutationFn: async (value: string) => {
      if (mode === "token") {
        await signIn(value);
      } else {
        await setName(value);
      }
      return getSession();
    },
    onSuccess: (session) => {
      queryClient.setQueryData(keys.session(), session);
      void router.navigate({ href: safeRedirect(redirect), replace: true });
    },
  });
  const props = {
    pending: submit.isPending,
    error: submit.isError ? signInErrorMessage(submit.error) : undefined,
    onSubmit: (value: string) => {
      submit.mutate(value);
    },
  };
  return (
    <main className="flex min-h-screen items-center justify-center bg-bg px-24">
      {mode === "token" ? <TokenCard {...props} /> : <NameCard {...props} />}
    </main>
  );
}
