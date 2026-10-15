import { ApiError, toApiError } from "./errors";

interface Answer<T> {
  data?: T;
  error?: unknown;
  response: Response;
}

/** openapi-fetch reports a failure as a value; the seam throws it so queries and mutations see an `ApiError`. A body that is not the error envelope has no code to branch on and becomes `daemon_unavailable`. */
export async function unwrap<T>(answer: Promise<Answer<T>>): Promise<T> {
  const { data, error, response } = await answer;
  if (error !== undefined || data === undefined) {
    throw (
      toApiError(error) ??
      new ApiError(
        "daemon_unavailable",
        `the daemon answered ${String(response.status)}`,
      )
    );
  }
  return data;
}

/** The same for a route that answers 204 and has no body to read. */
export async function expectOk(answer: Promise<Answer<unknown>>) {
  const { error, response } = await answer;
  if (error !== undefined || !response.ok) {
    throw (
      toApiError(error) ??
      new ApiError(
        "daemon_unavailable",
        `the daemon answered ${String(response.status)}`,
      )
    );
  }
}
