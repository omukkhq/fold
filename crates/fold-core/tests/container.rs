use fold_core::container::{ContainerFacts, ItemFact, completes_by_children};

const KINDS: [ItemFact; 4] = [
    ItemFact::Step { ticked: false },
    ItemFact::Step { ticked: true },
    ItemFact::Linked { child_done: false },
    ItemFact::Linked { child_done: true },
];

/// Every item list of up to `max` items over the four kinds, in every order.
#[cfg(test)]
fn item_lists(max: usize) -> Vec<Vec<ItemFact>> {
    let mut lists = vec![Vec::new()];
    let mut frontier = vec![Vec::new()];
    for _ in 0..max {
        let mut next = Vec::new();
        for list in &frontier {
            for kind in KINDS {
                let mut longer: Vec<ItemFact> = list.clone();
                longer.push(kind);
                next.push(longer);
            }
        }
        lists.extend(next.iter().cloned());
        frontier = next;
    }
    lists
}

/// The rule written as counts, independent of the implementation's single pass.
#[cfg(test)]
fn expected(facts: &ContainerFacts) -> bool {
    let linked = facts
        .items
        .iter()
        .filter(|item| matches!(item, ItemFact::Linked { .. }))
        .count();
    let open_steps = facts
        .items
        .iter()
        .filter(|item| matches!(item, ItemFact::Step { ticked: false }))
        .count();
    let open_children = facts
        .items
        .iter()
        .filter(|item| matches!(item, ItemFact::Linked { child_done: false }))
        .count();
    linked > 0
        && open_steps == 0
        && open_children == 0
        && !facts.ever_had_workspace
        && !facts.completed
        && !facts.archived
}

#[test]
fn every_combination_of_items_workspace_and_closure_matches_the_rule() {
    let mut checked = 0;
    let mut completing = 0;
    for items in item_lists(4) {
        for bits in 0..8u8 {
            let facts = ContainerFacts {
                items: items.clone(),
                ever_had_workspace: bits & 1 != 0,
                completed: bits & 2 != 0,
                archived: bits & 4 != 0,
            };
            let result = completes_by_children(&facts);
            assert_eq!(result, expected(&facts), "{facts:?}");
            checked += 1;
            completing += usize::from(result);
        }
    }
    // 1 + 4 + 16 + 64 + 256 item lists, each with the eight flag combinations.
    assert_eq!(checked, 341 * 8);
    assert!(completing > 0);
}

#[test]
fn linked_children_all_done_complete_the_parent() {
    let facts = ContainerFacts {
        items: vec![
            ItemFact::Linked { child_done: true },
            ItemFact::Linked { child_done: true },
        ],
        ..ContainerFacts::default()
    };
    assert!(completes_by_children(&facts));
}

#[test]
fn ticked_steps_do_not_hold_the_parent_back() {
    let facts = ContainerFacts {
        items: vec![
            ItemFact::Step { ticked: true },
            ItemFact::Linked { child_done: true },
        ],
        ..ContainerFacts::default()
    };
    assert!(completes_by_children(&facts));
}

#[test]
fn an_unticked_step_keeps_the_parent_open() {
    let facts = ContainerFacts {
        items: vec![
            ItemFact::Linked { child_done: true },
            ItemFact::Step { ticked: false },
        ],
        ..ContainerFacts::default()
    };
    assert!(!completes_by_children(&facts));
}

#[test]
fn one_unfinished_child_keeps_the_parent_open() {
    let facts = ContainerFacts {
        items: vec![
            ItemFact::Linked { child_done: true },
            ItemFact::Linked { child_done: false },
        ],
        ..ContainerFacts::default()
    };
    assert!(!completes_by_children(&facts));
}

#[test]
fn plain_steps_alone_never_complete() {
    for items in [vec![], vec![ItemFact::Step { ticked: true }]] {
        let facts = ContainerFacts {
            items,
            ..ContainerFacts::default()
        };
        assert!(!completes_by_children(&facts));
    }
}

#[test]
fn a_workspace_completion_or_archive_turns_the_rule_off() {
    let done = vec![ItemFact::Linked { child_done: true }];
    for facts in [
        ContainerFacts {
            items: done.clone(),
            ever_had_workspace: true,
            ..ContainerFacts::default()
        },
        ContainerFacts {
            items: done.clone(),
            completed: true,
            ..ContainerFacts::default()
        },
        ContainerFacts {
            items: done,
            archived: true,
            ..ContainerFacts::default()
        },
    ] {
        assert!(!completes_by_children(&facts), "{facts:?}");
    }
}
