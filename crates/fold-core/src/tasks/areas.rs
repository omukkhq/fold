use std::collections::BTreeSet;

use serde_json::json;

use crate::agents::{AgentName, AreaName, WorkerName};
use crate::error::{FoldError, codes};
use crate::ids::TaskId;
use crate::messages::Address;
use crate::tasks::LabelName;

/// The areas whose label is among `labels`, in the order of `areas`. Each area is given as its name and the label that puts a task in it.
pub fn areas_among<'a>(
    labels: &[LabelName],
    areas: &'a [(AreaName, LabelName)],
) -> Vec<&'a (AreaName, LabelName)> {
    areas
        .iter()
        .filter(|(_, label)| labels.contains(label))
        .collect()
}

/// The task is in the built-in area Main: the main agent builds it, and a human approves its proposal, or the main agent when the user said so for the task.
pub fn in_main_area(labels: &[LabelName]) -> bool {
    labels.iter().any(LabelName::is_main)
}

/// Main has no lead and no profile, because its tasks go to the main agent itself. Starting a lead for it and changing its profile are refused with `main_area`.
pub fn check_led_area(area: &AreaName) -> Result<(), FoldError> {
    if !area.is_main() {
        return Ok(());
    }
    Err(FoldError::new(
        codes::MAIN_AREA,
        "the area main belongs to the main agent: it has no lead and no profile",
    )
    .with_details([("area", json!(area.as_str()))])
    .with_hint("the main agent builds a task of main itself with claim_task; give work for a lead another area's label with update_task"))
}

/// The labels carry the labels of two or more areas. [`check_one_area`] refuses such a set on every write, but the database can hold one; the views flag it so a person can choose its area.
pub fn areas_conflict(labels: &[LabelName], areas: &[(AreaName, LabelName)]) -> bool {
    areas_among(labels, areas).len() > 1
}

/// A task belongs to at most one area, so the lead of exactly that area is offered it and may claim it; a task without an area label waits for the main agent to choose one. Labels that belong to no area are free in any number. A label set with two or more area labels is refused with `invalid_request` and reason `two_areas`, naming the areas.
pub fn check_one_area(
    labels: &[LabelName],
    areas: &[(AreaName, LabelName)],
) -> Result<(), FoldError> {
    let found = areas_among(labels, areas);
    if found.len() < 2 {
        return Ok(());
    }
    let named: Vec<String> = found
        .iter()
        .map(|(area, label)| format!("{label} (area {area})"))
        .collect();
    let choices: Vec<String> = found
        .iter()
        .map(|(area, label)| format!("{label} for {area}"))
        .collect();
    Err(FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "a task has at most one area label, and {} are area labels",
            named.join(", ")
        ),
    )
    .with_details([
        ("reason", json!("two_areas")),
        (
            "areas",
            json!(
                found
                    .iter()
                    .map(|(area, _)| area.as_str())
                    .collect::<Vec<_>>()
            ),
        ),
        (
            "labels",
            json!(
                found
                    .iter()
                    .map(|(_, label)| label.as_str())
                    .collect::<Vec<_>>()
            ),
        ),
    ])
    .with_hint(format!(
        "call update_task, or create_task or create_task_plan for new work, with labels that keep the label of one area ({}) and leave out the others; without an area label the main agent chooses the area",
        choices.join(" or ")
    )))
}

/// Defining area `name` over `label` must not give a task a second area, so it is refused with `invalid_request` and reason `two_areas` while a task in `tasks` carries `label` together with the label of an existing area in `areas`. `tasks` are the tasks that can still change, each with its labels: an archived task is read-only, so it is left out and never blocks a new area.
pub fn check_new_area(
    name: &AreaName,
    label: &LabelName,
    tasks: &[(TaskId, Vec<LabelName>)],
    areas: &[(AreaName, LabelName)],
) -> Result<(), FoldError> {
    let mut conflicted = Vec::new();
    let mut others = BTreeSet::new();
    for (task, labels) in tasks {
        if !labels.contains(label) {
            continue;
        }
        let found = areas_among(labels, areas);
        if found.is_empty() {
            continue;
        }
        conflicted.push(*task);
        others.extend(found.into_iter().map(|(area, _)| area.as_str()));
    }
    if conflicted.is_empty() {
        return Ok(());
    }
    Err(FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "area {name} would give a second area to the tasks that carry label {label} and the label of another area"
        ),
    )
    .with_details([
        ("reason", json!("two_areas")),
        (
            "tasks",
            json!(
                conflicted
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            ),
        ),
        ("areas", json!(others)),
    ])
    .with_hint(
        "leave one area label on each task in details.tasks with update_task, then call define_area again",
    ))
}

/// Why a reader is offered for a human's feedback on a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderRole {
    ClaimHolder,
    AreaLead,
    MainAgent,
}

/// A reader of a human's feedback on a task, as a worker name: an agent reads it through its main worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackReader {
    pub name: WorkerName,
    pub role: ReaderRole,
}

/// The readers a human's feedback on a task goes to, the default first: the worker that holds the implementation claim, the lead of the task's area and the main agents. Only they answer for the task, so no other agent is offered. A task of Main has no lead but the main agent, and a task in no area has none. Each worker is listed once, under its first role. `areas` gives each area's name, label and lead.
pub fn feedback_readers(
    holder: Option<&WorkerName>,
    labels: &[LabelName],
    areas: &[(AreaName, LabelName, Option<AgentName>)],
    main_agents: &[AgentName],
) -> Vec<FeedbackReader> {
    let main = in_main_area(labels);
    let leads = areas
        .iter()
        .filter(|(area, label, _)| !main && !area.is_main() && labels.contains(label))
        .filter_map(|(_, _, lead)| lead.as_ref());
    let mut readers: Vec<FeedbackReader> = Vec::new();
    let candidates = holder
        .map(|holder| (holder.clone(), ReaderRole::ClaimHolder))
        .into_iter()
        .chain(leads.map(|lead| (WorkerName::main(lead), ReaderRole::AreaLead)))
        .chain(
            main_agents
                .iter()
                .map(|agent| (WorkerName::main(agent), ReaderRole::MainAgent)),
        );
    for (name, role) in candidates {
        if readers.iter().all(|reader| reader.name != name) {
            readers.push(FeedbackReader { name, role });
        }
    }
    readers
}

/// The addresses a human's feedback on `task` goes to: `to` when every entry names one of the `readers`, as [`feedback_readers`] lists them, otherwise the first reader. A tag, a human or any other agent is refused with `recipient_not_reader`, and a task without readers with `recipient_not_found` when `to` is left out.
pub fn feedback_to(
    task: TaskId,
    to: Option<Vec<Address>>,
    readers: &[FeedbackReader],
) -> Result<Vec<Address>, FoldError> {
    let Some(to) = to else {
        return readers
            .first()
            .map(|reader| vec![Address::of_worker(&reader.name)])
            .ok_or_else(|| {
                FoldError::new(
                    codes::RECIPIENT_NOT_FOUND,
                    format!("{task} has no reader to receive the feedback"),
                )
                .with_details([("task", json!(task.to_string()))])
                .with_hint("give the task an area whose lead runs, or give an agent the main tag")
            });
    };
    let reads = |address: &Address| match address {
        Address::Agent(agent) => {
            let name = WorkerName::main(agent);
            readers.iter().any(|reader| reader.name == name)
        }
        Address::Worker(worker) => readers.iter().any(|reader| &reader.name == worker),
        Address::Tag(_) | Address::All | Address::Human(_) | Address::Humans => false,
    };
    let refused: Vec<String> = to
        .iter()
        .filter(|address| !reads(address))
        .map(ToString::to_string)
        .collect();
    if refused.is_empty() {
        return Ok(to);
    }
    let offered: Vec<String> = readers
        .iter()
        .map(|reader| reader.name.to_string())
        .collect();
    Err(FoldError::new(
        codes::RECIPIENT_NOT_READER,
        format!("feedback on {task} goes only to its claim holder, its area lead or the main agent"),
    )
    .with_details([
        ("task", json!(task.to_string())),
        ("to", json!(refused)),
        ("readers", json!(offered)),
    ])
    .with_hint(format!("send it to a reader that GET /tasks/{task}/readers lists, or leave out `to` for the default reader")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(name: &str) -> LabelName {
        LabelName::parse(name).unwrap()
    }

    fn labels(names: &[&str]) -> Vec<LabelName> {
        names.iter().map(|name| label(name)).collect()
    }

    fn areas() -> Vec<(AreaName, LabelName)> {
        vec![
            (AreaName::parse("web").unwrap(), label("ui")),
            (AreaName::parse("api").unwrap(), label("server")),
        ]
    }

    fn task(number: u32) -> TaskId {
        TaskId::from_number(number).unwrap()
    }

    #[test]
    fn main_is_an_area_of_the_main_agent_without_a_lead() {
        assert!(in_main_area(&labels(&["docs", "main"])));
        assert!(!in_main_area(&labels(&["ui", "mainline"])));
        assert!(check_led_area(&AreaName::parse("web").unwrap()).is_ok());
        let error = check_led_area(&AreaName::main()).unwrap_err();
        assert_eq!(error.code(), codes::MAIN_AREA);
        assert_eq!(error.details().unwrap()["area"], "main");
        let mut with_main = areas();
        with_main.push((AreaName::main(), LabelName::main()));
        let error = check_one_area(&labels(&["main", "ui"]), &with_main).unwrap_err();
        assert_eq!(error.details().unwrap()["areas"], json!(["web", "main"]));
    }

    #[test]
    fn one_area_label_and_any_number_of_other_labels_pass() {
        let areas = areas();
        for set in [
            labels(&[]),
            labels(&["ui"]),
            labels(&["chore", "bug", "docs", "perf", "ui"]),
            labels(&["chore", "bug", "docs", "perf"]),
            labels(&["ui", "ui"]),
        ] {
            assert!(check_one_area(&set, &areas).is_ok(), "{set:?}");
            assert!(!areas_conflict(&set, &areas), "{set:?}");
        }
    }

    #[test]
    fn two_area_labels_are_refused_with_the_areas_named() {
        let areas = areas();
        let set = labels(&["chore", "server", "ui"]);
        assert!(areas_conflict(&set, &areas));
        let error = check_one_area(&set, &areas).unwrap_err();
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        let details = error.details().unwrap();
        assert_eq!(details["reason"], "two_areas");
        assert_eq!(details["areas"], json!(["web", "api"]));
        assert_eq!(details["labels"], json!(["ui", "server"]));
        assert!(error.message().contains("ui (area web), server (area api)"));
        assert!(
            error
                .hint()
                .unwrap()
                .contains("ui for web or server for api"),
            "{error:?}"
        );
        assert!(error.hint().unwrap().contains("update_task"), "{error:?}");
    }

    #[test]
    fn a_new_area_is_refused_while_its_label_shares_a_task_with_another_area() {
        let areas = areas();
        let docs = AreaName::parse("docs").unwrap();
        let tasks = vec![
            (task(1), labels(&["guide", "ui"])),
            (task(2), labels(&["guide", "chore"])),
            (task(3), labels(&["server", "guide"])),
            (task(4), labels(&["ui"])),
        ];
        let error = check_new_area(&docs, &label("guide"), &tasks, &areas).unwrap_err();
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        let details = error.details().unwrap();
        assert_eq!(details["reason"], "two_areas");
        assert_eq!(details["tasks"], json!(["T-1", "T-3"]));
        assert_eq!(details["areas"], json!(["api", "web"]));
        assert!(error.hint().unwrap().contains("update_task"));

        assert!(check_new_area(&docs, &label("guide"), &tasks[1..2], &areas).is_ok());
        assert!(check_new_area(&docs, &label("chore"), &tasks, &areas).is_ok());
        assert!(check_new_area(&docs, &label("guide"), &tasks, &[]).is_ok());
    }

    fn worker(name: &str) -> WorkerName {
        WorkerName::parse(name).unwrap()
    }

    fn agents(names: &[&str]) -> Vec<AgentName> {
        names
            .iter()
            .map(|name| AgentName::parse(name).unwrap())
            .collect()
    }

    fn led() -> Vec<(AreaName, LabelName, Option<AgentName>)> {
        vec![
            (
                AreaName::parse("web").unwrap(),
                label("ui"),
                AgentName::parse("web-lead").ok(),
            ),
            (AreaName::parse("api").unwrap(), label("server"), None),
            (AreaName::main(), LabelName::main(), None),
        ]
    }

    fn readers(
        holder: Option<&str>,
        set: &[&str],
        main_agents: &[&str],
    ) -> Vec<(String, ReaderRole)> {
        let holder = holder.map(worker);
        feedback_readers(holder.as_ref(), &labels(set), &led(), &agents(main_agents))
            .into_iter()
            .map(|reader| (reader.name.to_string(), reader.role))
            .collect()
    }

    #[test]
    fn feedback_goes_to_the_claim_holder_first_then_the_area_lead_and_the_main_agent() {
        assert_eq!(
            readers(Some("web-t12"), &["ui", "bug"], &["main"]),
            [
                ("web-t12".to_owned(), ReaderRole::ClaimHolder),
                ("web-lead".to_owned(), ReaderRole::AreaLead),
                ("main".to_owned(), ReaderRole::MainAgent),
            ]
        );
    }

    #[test]
    fn without_a_claim_the_area_lead_is_the_default() {
        assert_eq!(
            readers(None, &["ui"], &["main"]),
            [
                ("web-lead".to_owned(), ReaderRole::AreaLead),
                ("main".to_owned(), ReaderRole::MainAgent),
            ]
        );
    }

    #[test]
    fn a_task_in_no_area_or_in_an_area_without_a_lead_goes_to_the_main_agent() {
        for set in [&["bug"][..], &[], &["server"]] {
            assert_eq!(
                readers(None, set, &["main"]),
                [("main".to_owned(), ReaderRole::MainAgent)],
                "{set:?}"
            );
        }
        assert!(readers(None, &["bug"], &[]).is_empty());
    }

    #[test]
    fn a_task_of_main_has_the_main_agent_alone_beside_its_holder() {
        assert_eq!(
            readers(None, &["main", "ui"], &["main"]),
            [("main".to_owned(), ReaderRole::MainAgent)]
        );
        assert_eq!(
            readers(Some("main"), &["main"], &["main"]),
            [("main".to_owned(), ReaderRole::ClaimHolder)],
            "the main agent that holds the claim is listed once"
        );
    }

    #[test]
    fn a_lead_that_holds_the_claim_is_listed_once_and_its_child_worker_apart() {
        assert_eq!(
            readers(Some("web-lead"), &["ui"], &["main"]),
            [
                ("web-lead".to_owned(), ReaderRole::ClaimHolder),
                ("main".to_owned(), ReaderRole::MainAgent),
            ]
        );
        assert_eq!(
            readers(Some("web-lead/ui"), &["ui"], &["web-lead"]),
            [
                ("web-lead/ui".to_owned(), ReaderRole::ClaimHolder),
                ("web-lead".to_owned(), ReaderRole::AreaLead),
            ]
        );
    }

    #[test]
    fn a_task_with_the_labels_of_two_led_areas_offers_both_leads() {
        let areas = vec![
            (
                AreaName::parse("web").unwrap(),
                label("ui"),
                AgentName::parse("web-lead").ok(),
            ),
            (
                AreaName::parse("api").unwrap(),
                label("server"),
                AgentName::parse("api-lead").ok(),
            ),
        ];
        let names: Vec<(String, ReaderRole)> =
            feedback_readers(None, &labels(&["server", "ui"]), &areas, &agents(&["main"]))
                .into_iter()
                .map(|reader| (reader.name.to_string(), reader.role))
                .collect();
        assert_eq!(
            names,
            [
                ("web-lead".to_owned(), ReaderRole::AreaLead),
                ("api-lead".to_owned(), ReaderRole::AreaLead),
                ("main".to_owned(), ReaderRole::MainAgent),
            ]
        );
    }

    fn t12() -> TaskId {
        "T-12".parse().unwrap()
    }

    fn addresses(entries: &[&str]) -> Vec<Address> {
        Address::parse_all(entries).unwrap()
    }

    fn offered(holder: Option<&str>) -> Vec<FeedbackReader> {
        let holder = holder.map(worker);
        feedback_readers(
            holder.as_ref(),
            &labels(&["ui"]),
            &led(),
            &agents(&["main"]),
        )
    }

    #[test]
    fn feedback_without_recipients_goes_to_the_first_reader() {
        assert_eq!(
            feedback_to(t12(), None, &offered(Some("web-lead/ui"))).unwrap(),
            addresses(&["web-lead/ui"])
        );
        assert_eq!(
            feedback_to(t12(), None, &offered(None)).unwrap(),
            addresses(&["web-lead"]),
            "without a claim the area lead"
        );
        let none = feedback_to(t12(), None, &[]).unwrap_err();
        assert_eq!(none.code(), codes::RECIPIENT_NOT_FOUND);
    }

    #[test]
    fn feedback_takes_recipients_that_are_readers_by_agent_or_worker_name() {
        let readers = offered(Some("web-lead/ui"));
        let to = addresses(&["web-lead/ui", "web-lead", "main"]);
        assert_eq!(feedback_to(t12(), Some(to.clone()), &readers).unwrap(), to);
    }

    #[test]
    fn feedback_to_anyone_but_a_reader_is_refused() {
        let readers = offered(Some("web-lead/ui"));
        for entry in [
            "backend",
            "web-lead/other",
            "tag:ui",
            "all",
            "humans",
            "human:ana",
        ] {
            let refused =
                feedback_to(t12(), Some(addresses(&["main", entry])), &readers).unwrap_err();
            assert_eq!(refused.code(), codes::RECIPIENT_NOT_READER, "{entry}");
            assert_eq!(
                refused.details().and_then(|details| details.get("to")),
                Some(&json!([entry])),
                "{entry}"
            );
        }
    }
}
