import type { components } from "./schema";

type ErrorBody = components["schemas"]["ErrorBody"];

/** The part of the error envelope that is enough to branch on: a proxy can answer with a code and no message. */
interface CodedBody {
  error: Pick<ErrorBody, "code"> & Record<string, unknown>;
}

/** A failed request, thrown so that queries and the event stream see it. `code` is the envelope's stable code. */
export class ApiError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = "ApiError";
    this.code = code;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isCodedBody(body: unknown): body is CodedBody {
  return (
    isRecord(body) &&
    isRecord(body.error) &&
    typeof body.error.code === "string"
  );
}

/** The `ApiError` of a response body in the error envelope, or null for any other body. */
export function toApiError(body: unknown): ApiError | null {
  if (!isCodedBody(body)) {
    return null;
  }
  const { code, message } = body.error;
  return new ApiError(code, typeof message === "string" ? message : "");
}

/** The stable code of a thrown `ApiError` or of a response body in the error envelope, or null for any other value. Screens branch on it, never on the message. */
export function apiErrorCode(error: unknown): string | null {
  if (error instanceof ApiError) {
    return error.code;
  }
  return isCodedBody(error) ? error.error.code : null;
}

export function isApiError(error: unknown, code: string): boolean {
  return apiErrorCode(error) === code;
}
