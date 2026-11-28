use fold_daemon::mcp::{FooterCounts, FooterPart, footer};

#[cfg(test)]
fn counts(unread: u64, new_events: u64, claims_near_stale: u64) -> FooterCounts {
    FooterCounts::default()
        .with(FooterPart::Unread, unread)
        .with(FooterPart::NewEvents, new_events)
        .with(FooterPart::ClaimsNearStale, claims_near_stale)
}

#[cfg(test)]
type Row = ((u64, u64, u64), Option<&'static str>);

#[test]
fn each_combination_of_counts_gives_its_exact_line() {
    let table: &[Row] = &[
        ((0, 0, 0), None),
        ((2, 0, 0), Some("fold: 2 unread")),
        ((0, 5, 0), Some("fold: 5 new events")),
        ((0, 0, 1), Some("fold: 1 claims near stale")),
        ((2, 5, 0), Some("fold: 2 unread · 5 new events")),
        ((2, 0, 1), Some("fold: 2 unread · 1 claims near stale")),
        ((0, 5, 1), Some("fold: 5 new events · 1 claims near stale")),
        (
            (2, 5, 1),
            Some("fold: 2 unread · 5 new events · 1 claims near stale"),
        ),
        (
            (1000, 1, 70),
            Some("fold: 1000 unread · 1 new events · 70 claims near stale"),
        ),
        ((0, 100, 0), Some("fold: 100 new events")),
        ((0, 101, 0), Some("fold: 100+ new events")),
        (
            (101, 0, 101),
            Some("fold: 101 unread · 101 claims near stale"),
        ),
    ];
    for ((unread, new_events, stale), line) in table {
        assert_eq!(
            footer(&counts(*unread, *new_events, *stale)).as_deref(),
            *line,
            "counts ({unread}, {new_events}, {stale})"
        );
    }
}

#[test]
fn the_order_on_the_line_is_fixed_whatever_the_order_of_the_counts() {
    let reversed = FooterCounts::default()
        .with(FooterPart::Held, 6)
        .with(FooterPart::WaitForUser, 5)
        .with(FooterPart::QuestionsForUser, 4)
        .with(FooterPart::ClaimsNearStale, 3)
        .with(FooterPart::NewEvents, 2)
        .with(FooterPart::Unread, 1);
    assert_eq!(
        footer(&reversed).as_deref(),
        Some(
            "fold: 1 unread · 2 new events · 3 claims near stale · 4 questions for the user · 5 wait for the user · 6 held"
        )
    );
    assert_eq!(
        FooterPart::ORDER,
        [
            FooterPart::Unread,
            FooterPart::NewEvents,
            FooterPart::ClaimsNearStale,
            FooterPart::QuestionsForUser,
            FooterPart::WaitForUser,
            FooterPart::Held
        ]
    );
}

#[test]
fn the_items_that_wait_for_the_user_show_alone_as_their_own_part() {
    let counts = FooterCounts::default().with(FooterPart::WaitForUser, 3);
    assert_eq!(
        footer(&counts).as_deref(),
        Some("fold: 3 wait for the user")
    );
}

#[test]
fn a_question_and_a_hold_are_counted_apart_with_the_question_first() {
    let counts = FooterCounts::default()
        .with(FooterPart::Held, 1)
        .with(FooterPart::QuestionsForUser, 1);
    assert_eq!(
        footer(&counts).as_deref(),
        Some("fold: 1 questions for the user · 1 held")
    );
}

#[test]
fn the_counts_of_two_sources_add_up_part_by_part() {
    let one = FooterCounts::default()
        .with(FooterPart::Unread, 2)
        .with(FooterPart::Held, 1);
    let other = FooterCounts::default()
        .with(FooterPart::Unread, 3)
        .with(FooterPart::NewEvents, u64::MAX);
    let sum = one
        .plus(other)
        .plus(FooterCounts::default().with(FooterPart::NewEvents, 1));
    assert_eq!(sum.get(FooterPart::Unread), 5);
    assert_eq!(sum.get(FooterPart::Held), 1);
    assert_eq!(sum.get(FooterPart::NewEvents), u64::MAX);
}

#[test]
fn a_later_count_for_the_same_part_replaces_the_earlier_one() {
    let counts = FooterCounts::default()
        .with(FooterPart::Unread, 4)
        .with(FooterPart::Unread, 0);
    assert_eq!(counts.get(FooterPart::Unread), 0);
    assert_eq!(footer(&counts), None);
    assert_eq!(footer(&FooterCounts::default()), None);
}
