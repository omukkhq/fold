import { z } from "zod";
import { apiErrorCode } from "@/api/errors";
import type { RoleEntry, StartWorkerBody } from "@/api/workers";
import { failureText } from "@/format/failure";

/** The Claude Code model aliases the dialog offers; an empty choice leaves the role's model from the project config. */
export const MODELS = ["opus", "sonnet", "haiku"] as const;

export const startLeadSchema = z
  .object({
    role: z.enum(["area_lead", "review_lead"]),
    area: z.string(),
    model: z.string(),
    instructions: z.string(),
  })
  .refine((values) => values.role === "review_lead" || values.area !== "", {
    path: ["area"],
    message: "Choose the area the lead owns.",
  });

export type StartLeadValues = z.infer<typeof startLeadSchema>;

export function startLeadDefaults(areas: readonly string[]): StartLeadValues {
  return {
    role: areas.length === 0 ? "review_lead" : "area_lead",
    area: areas[0] ?? "",
    model: "",
    instructions: "",
  };
}

/** The request body: a review lead has no area, and an empty model or text is left out. */
export function startLeadBody(values: StartLeadValues): StartWorkerBody {
  const body: StartWorkerBody = { role: values.role };
  if (values.role === "area_lead") body.area = values.area;
  if (values.model !== "") body.model = values.model;
  const instructions = values.instructions.trim();
  if (instructions !== "") body.instructions = instructions;
  return body;
}

/** The cache time a start gives the role, from the project config; a start does not choose it. */
export function roleCacheTtl(
  roles: readonly RoleEntry[],
  role: StartLeadValues["role"],
): string | null {
  return roles.find((entry) => entry.role === role)?.cache_ttl ?? null;
}

export const LEAD_DESCRIPTIONS: Record<StartLeadValues["role"], string> = {
  area_lead:
    "A lead is a long-lived agent. It splits its tasks and starts short-lived task coders; fold stops each coder when its work is done.",
  review_lead:
    "The review lead is a long-lived agent. It groups open proposals into review batches and may start short-lived batch reviewers; fold stops each reviewer when its batch is done.",
};

export interface Failure {
  text: string;
  code: string | null;
}

/** A refused start, worded for its code. */
export function startFailure(error: unknown, values: StartLeadValues): Failure {
  const code = apiErrorCode(error);
  const area = values.role === "area_lead" ? values.area : "";
  switch (code) {
    case "area_has_lead":
      return {
        code,
        text: `A lead for the ${area} area is already running. Stop it first, or choose another area.`,
      };
    case "worker_exists":
      return {
        code,
        text: "A worker with this name is already running. Stop it first.",
      };
    case "worker_limit_reached":
      return {
        code,
        text: "fold runs as many workers as the project allows. Stop one first, or raise the limit in .fold/config.toml.",
      };
    case "area_not_found":
      return { code, text: `The project has no area named ${area}.` };
    case "invalid_config":
      return {
        code,
        text: "fold cannot read the worker settings in .fold/config.toml. Run fld doctor.",
      };
    case "worker_start_denied":
      return { code, text: "A human starts only the leads." };
    default:
      return { code, text: failureText(error) };
  }
}

/** A refused stop, worded for its code. */
export function stopFailure(error: unknown): Failure {
  const code = apiErrorCode(error);
  if (code === "worker_process_not_found") {
    return { code, text: "fold runs no process for this worker." };
  }
  return { code, text: failureText(error) };
}
