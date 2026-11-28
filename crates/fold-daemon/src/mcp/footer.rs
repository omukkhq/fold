use std::future::Future;
use std::pin::Pin;

use crate::services::agents::SessionContext;
use crate::services::events::NEW_EVENTS_CAP;

/// One count of the footer. The order of [`FooterPart::ORDER`] is the order on the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterPart {
    Unread,
    NewEvents,
    ClaimsNearStale,
    /// The open questions to the humans, counted for the main agent only.
    QuestionsForUser,
    /// The other items that wait for the user, counted for the main agent only.
    WaitForUser,
    /// The tasks on hold, counted for the main agent only: they wait until a human takes the hold off, not for an answer.
    Held,
}

impl FooterPart {
    pub const ORDER: [FooterPart; 6] = [
        FooterPart::Unread,
        FooterPart::NewEvents,
        FooterPart::ClaimsNearStale,
        FooterPart::QuestionsForUser,
        FooterPart::WaitForUser,
        FooterPart::Held,
    ];

    fn label(self) -> &'static str {
        match self {
            FooterPart::Unread => "unread",
            FooterPart::NewEvents => "new events",
            FooterPart::ClaimsNearStale => "claims near stale",
            FooterPart::QuestionsForUser => "questions for the user",
            FooterPart::WaitForUser => "wait for the user",
            FooterPart::Held => "held",
        }
    }

    fn index(self) -> usize {
        match self {
            FooterPart::Unread => 0,
            FooterPart::NewEvents => 1,
            FooterPart::ClaimsNearStale => 2,
            FooterPart::QuestionsForUser => 3,
            FooterPart::WaitForUser => 4,
            FooterPart::Held => 5,
        }
    }
}

/// The counts one tool result reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FooterCounts([u64; 6]);

impl FooterCounts {
    #[must_use]
    pub fn with(mut self, part: FooterPart, count: u64) -> Self {
        self.0[part.index()] = count;
        self
    }

    pub fn get(&self, part: FooterPart) -> u64 {
        self.0[part.index()]
    }

    /// The part-by-part sum of both counts.
    #[must_use]
    pub fn plus(mut self, other: FooterCounts) -> Self {
        for (count, more) in self.0.iter_mut().zip(other.0) {
            *count = count.saturating_add(more);
        }
        self
    }
}

/// The last line of a tool result, such as `fold: 2 unread · 1 claims near stale`. A zero part is left out, and there is no line when every part is zero, so an agent sees the footer only when something waits for it. The new events are counted only up to [`NEW_EVENTS_CAP`], so a larger count shows as `100+`.
pub fn footer(ctx: &FooterCounts) -> Option<String> {
    let parts: Vec<String> = FooterPart::ORDER
        .iter()
        .filter(|part| ctx.get(**part) > 0)
        .map(|part| match ctx.get(*part) {
            count if *part == FooterPart::NewEvents && count > NEW_EVENTS_CAP => {
                format!("{NEW_EVENTS_CAP}+ {}", part.label())
            }
            count => format!("{count} {}", part.label()),
        })
        .collect();
    (!parts.is_empty()).then(|| format!("fold: {}", parts.join(" · ")))
}

/// A domain that puts its counts in the footer of every tool result. A source that cannot count reports zero and logs why.
pub trait FooterSource: Send + Sync {
    /// True for a count taken as the call arrives, before the tool runs and the call is recorded; the other counts are taken after.
    fn before_call(&self) -> bool {
        false
    }

    fn count<'a>(
        &'a self,
        session: &'a SessionContext,
    ) -> Pin<Box<dyn Future<Output = FooterCounts> + Send + 'a>>;
}

/// The counts of a source that gives the one part `part`, counted by `count`.
pub fn single<'a>(
    part: FooterPart,
    count: impl Future<Output = u64> + Send + 'a,
) -> Pin<Box<dyn Future<Output = FooterCounts> + Send + 'a>> {
    Box::pin(async move { FooterCounts::default().with(part, count.await) })
}
