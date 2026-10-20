use std::collections::BTreeSet;

use serde_json::json;

use crate::agents::AreaName;
use crate::error::{FoldError, codes};
use crate::ids::TaskId;
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
}
