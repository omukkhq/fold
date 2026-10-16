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
        .with(FooterPart::ClaimsNearStale, 3)
        .with(FooterPart::NewEvents, 2)
        .with(FooterPart::Unread, 1);
    assert_eq!(
        footer(&reversed).as_deref(),
        Some("fold: 1 unread · 2 new events · 3 claims near stale")
    );
    assert_eq!(
        FooterPart::ORDER,
        [
            FooterPart::Unread,
            FooterPart::NewEvents,
            FooterPart::ClaimsNearStale
        ]
    );
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
