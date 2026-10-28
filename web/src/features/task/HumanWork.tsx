import { Check, UserRound } from "lucide-react";
import { completeTask, type TaskDetail } from "@/api/tasks";
import { Button } from "@/components/button/Button";
import { humanWork } from "./humanTask";
import { WriteError } from "./parts";
import { useTaskWrite } from "./useTaskWrite";

/** The Done control of a human task: a person does the work outside fold, and a human marks it Done here. */
export function HumanWork({ slug, task }: { slug: string; task: TaskDetail }) {
  const done = useTaskWrite(slug, task.task, () =>
    completeTask(slug, task.task),
  );
  const work = humanWork(task);
  if (work === null) {
    return null;
  }
  const showButton = work.state === "open" || work.state === "waits";
  return (
    <section
      aria-label="Human task"
      className="flex flex-col gap-12 rounded-m bg-sunken px-16 py-14 inset-ring inset-ring-line"
    >
      <h2 className="flex items-center gap-8 text-14 font-semibold text-text">
        <UserRound aria-hidden="true" size={15} className="text-text-2" />
        Human task
      </h2>
      <p className="text-14 leading-normal text-text-2">{work.note}</p>
      {showButton ? (
        <div className="flex items-center gap-10">
          <Button
            variant="primary"
            icon={<Check aria-hidden="true" size={15} />}
            disabled={!work.canMarkDone}
            pending={done.isPending}
            pendingLabel="Marking Done…"
            onClick={() => {
              done.mutate(undefined);
            }}
          >
            Mark Done
          </Button>
          {work.waitsFor.length > 0 ? (
            <span className="text-13 text-text-2">
              Waits for {work.waitsFor.join(", ")}
            </span>
          ) : null}
        </div>
      ) : null}
      <WriteError error={done.error} />
    </section>
  );
}
