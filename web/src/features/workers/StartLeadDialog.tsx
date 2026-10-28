import { zodResolver } from "@hookform/resolvers/zod";
import * as RadixDialog from "@radix-ui/react-dialog";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Play, X } from "lucide-react";
import { useId, useState } from "react";
import { Controller, useForm, useWatch } from "react-hook-form";
import type { BoardLabel } from "@/api/board";
import { keys } from "@/api/keys";
import { startWorker, type AreaEntry, type RoleEntry } from "@/api/workers";
import { Button, IconButton } from "@/components/button/Button";
import { Select } from "@/components/form/controls";
import { controlClasses } from "@/components/form/Field";
import { Label } from "@/components/label/Label";
import { menuContent, menuItem } from "@/components/overlay/menuStyles";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import { ErrorNote } from "./parts";
import {
  LEAD_DESCRIPTIONS,
  MODELS,
  defaultModelText,
  startFailure,
  startLeadBody,
  startLeadDefaults,
  startLeadSchema,
  type Failure,
  type StartLeadValues,
} from "./startLead";
import { areaLabel } from "./team";

const ROLES = [
  { value: "area_lead", label: "Area lead" },
  { value: "review_lead", label: "Review lead" },
] as const;

const FIELD_LABEL = "text-14 leading-tight font-semibold text-text";

export function StartLeadDialog({
  slug,
  open,
  onOpenChange,
  areas,
  roles,
  labels,
}: {
  slug: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  areas: readonly AreaEntry[];
  roles: readonly RoleEntry[];
  labels: readonly BoardLabel[];
}) {
  const queryClient = useQueryClient();
  const ids = useId();
  const [failure, setFailure] = useState<Failure | null>(null);
  const {
    control,
    handleSubmit,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<StartLeadValues>({
    resolver: zodResolver(startLeadSchema),
    defaultValues: startLeadDefaults(areas.map((a) => a.name)),
  });
  const role = useWatch({ control, name: "role" });

  const close = () => {
    reset(startLeadDefaults(areas.map((a) => a.name)));
    setFailure(null);
    onOpenChange(false);
  };

  const submit = handleSubmit(async (values) => {
    setFailure(null);
    try {
      await startWorker(slug, startLeadBody(values));
      await queryClient.invalidateQueries({ queryKey: keys.workers(slug) });
      close();
    } catch (error) {
      setFailure(startFailure(error, values));
    }
  });

  return (
    <RadixDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next && !isSubmitting) close();
      }}
    >
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 bg-scrim" />
        <RadixDialog.Content className="fixed top-1/2 left-1/2 flex max-h-full w-request-dialog max-w-full -translate-x-1/2 -translate-y-1/2 flex-col overflow-y-auto rounded-l bg-surface shadow-dialog outline-none">
          <div className="flex items-center justify-between border-b border-line py-12 pr-16 pl-24">
            <RadixDialog.Title className="text-16 leading-tight font-semibold text-text">
              Start a lead
            </RadixDialog.Title>
            <RadixDialog.Close asChild>
              <IconButton aria-label="Close" size="sm" disabled={isSubmitting}>
                <X aria-hidden="true" size={13} />
              </IconButton>
            </RadixDialog.Close>
          </div>
          <form
            noValidate
            onSubmit={(event) => void submit(event)}
            className="flex flex-col"
          >
            <div className="flex flex-col gap-16 px-24 pt-18 pb-20">
              <RadixDialog.Description className="text-14 text-text-2">
                {LEAD_DESCRIPTIONS[role]}
              </RadixDialog.Description>
              <Controller
                control={control}
                name="role"
                render={({ field }) => (
                  <fieldset className="flex flex-col gap-6">
                    <legend className={cn(FIELD_LABEL, "mb-6")}>Role</legend>
                    <div className="flex gap-2 self-start rounded-m bg-sunken p-3">
                      {ROLES.map((option) => (
                        <label
                          key={option.value}
                          className={cn(
                            "cursor-pointer rounded-s px-12 py-5 text-14 has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-accent",
                            field.value === option.value
                              ? "bg-surface font-semibold text-text inset-ring inset-ring-line"
                              : "font-medium text-text-2",
                          )}
                        >
                          <input
                            type="radio"
                            name={field.name}
                            value={option.value}
                            checked={field.value === option.value}
                            disabled={isSubmitting}
                            onChange={() => {
                              field.onChange(option.value);
                            }}
                            className="sr-only"
                          />
                          {option.label}
                        </label>
                      ))}
                    </div>
                  </fieldset>
                )}
              />
              {role === "area_lead" ? (
                <Controller
                  control={control}
                  name="area"
                  render={({ field }) => {
                    const chosen = areaLabel(areas, labels, field.value);
                    return (
                      <div className="flex flex-col gap-6">
                        <span id={`${ids}-area`} className={FIELD_LABEL}>
                          Area
                        </span>
                        <DropdownMenu.Root>
                          <DropdownMenu.Trigger asChild>
                            <button
                              type="button"
                              aria-labelledby={`${ids}-area`}
                              aria-invalid={errors.area ? true : undefined}
                              disabled={isSubmitting || areas.length === 0}
                              className={cn(
                                controlClasses,
                                focusRing,
                                "flex items-center gap-8 py-8 text-left",
                              )}
                            >
                              {chosen ? (
                                <Label color={chosen.color}>
                                  {chosen.name}
                                </Label>
                              ) : (
                                <span className="text-text-2">
                                  {areas.length === 0
                                    ? "No area yet: ask the main agent to define one"
                                    : "Choose an area"}
                                </span>
                              )}
                              <span className="flex-1" />
                              <ChevronDown
                                aria-hidden="true"
                                size={14}
                                className="text-text-2"
                              />
                            </button>
                          </DropdownMenu.Trigger>
                          <DropdownMenu.Portal>
                            <DropdownMenu.Content
                              align="start"
                              sideOffset={4}
                              className={cn(menuContent, "min-w-max")}
                            >
                              <DropdownMenu.RadioGroup
                                value={field.value}
                                onValueChange={field.onChange}
                              >
                                {areas.map((area) => {
                                  const label = areaLabel(
                                    areas,
                                    labels,
                                    area.name,
                                  );
                                  return (
                                    <DropdownMenu.RadioItem
                                      key={area.name}
                                      value={area.name}
                                      className={menuItem}
                                    >
                                      {label ? (
                                        <Label color={label.color}>
                                          {label.name}
                                        </Label>
                                      ) : null}
                                      <span className="text-text-2">
                                        {area.name}
                                      </span>
                                    </DropdownMenu.RadioItem>
                                  );
                                })}
                              </DropdownMenu.RadioGroup>
                            </DropdownMenu.Content>
                          </DropdownMenu.Portal>
                        </DropdownMenu.Root>
                        <p className="text-13 text-text-2">
                          The lead owns every task with this area label.
                        </p>
                        {errors.area ? (
                          <p className="text-13 font-medium text-bad">
                            {errors.area.message}
                          </p>
                        ) : null}
                      </div>
                    );
                  }}
                />
              ) : null}
              <Controller
                control={control}
                name="model"
                render={({ field }) => (
                  <Select
                    label="Model"
                    value={field.value}
                    onChange={field.onChange}
                    onBlur={field.onBlur}
                    disabled={isSubmitting}
                  >
                    <option value="">{defaultModelText(roles, role)}</option>
                    {MODELS.map((model) => (
                      <option key={model} value={model}>
                        {model}
                      </option>
                    ))}
                  </Select>
                )}
              />
              <div className="flex flex-col gap-6">
                <label htmlFor={`${ids}-instructions`} className={FIELD_LABEL}>
                  Instructions
                </label>
                <Controller
                  control={control}
                  name="instructions"
                  render={({ field }) => (
                    <MentionTextarea
                      id={`${ids}-instructions`}
                      slug={slug}
                      rows={4}
                      ref={field.ref}
                      value={field.value}
                      onValueChange={field.onChange}
                      onBlur={field.onBlur}
                      readOnly={isSubmitting}
                      className="resize-y"
                    />
                  )}
                />
              </div>
              {failure ? <ErrorNote failure={failure} /> : null}
            </div>
            <div className="flex items-center justify-end gap-8 border-t border-line px-24 py-12">
              <Button disabled={isSubmitting} onClick={close}>
                Cancel
              </Button>
              <Button
                type="submit"
                variant="primary"
                pending={isSubmitting}
                pendingLabel="Starting lead…"
                icon={<Play aria-hidden="true" size={15} />}
              >
                Start lead
              </Button>
            </div>
          </form>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
