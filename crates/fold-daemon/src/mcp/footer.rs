use std::future::Future;
use std::pin::Pin;

use crate::services::agents::SessionContext;

/// One count of the footer. The order of [`FooterPart::ORDER`] is the order on the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterPart {
    Unread,
    NewEvents,
    ClaimsNearStale,
}

impl FooterPart {
    pub const ORDER: [FooterPart; 3] = [
        FooterPart::Unread,
        FooterPart::NewEvents,
        FooterPart::ClaimsNearStale,
    ];

    fn label(self) -> &'static str {
        match self {
            FooterPart::Unread => "unread",
            FooterPart::NewEvents => "new events",
            FooterPart::ClaimsNearStale => "claims near stale",
        }
    }

    fn index(self) -> usize {
        match self {
            FooterPart::Unread => 0,
            FooterPart::NewEvents => 1,
            FooterPart::ClaimsNearStale => 2,
        }
    }
}

/// The three counts one tool result reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FooterCounts([u64; 3]);

impl FooterCounts {
    #[must_use]
    pub fn with(mut self, part: FooterPart, count: u64) -> Self {
        self.0[part.index()] = count;
        self
    }

    pub fn get(&self, part: FooterPart) -> u64 {
        self.0[part.index()]
    }
}

/// The last line of a tool result, such as `fold: 2 unread · 1 claims near stale`. A zero part is left out, and there is no line when every part is zero, so an agent sees the footer only when something waits for it.
pub fn footer(ctx: &FooterCounts) -> Option<String> {
    let parts: Vec<String> = FooterPart::ORDER
        .iter()
        .filter(|part| ctx.get(**part) > 0)
        .map(|part| format!("{} {}", ctx.get(*part), part.label()))
        .collect();
    (!parts.is_empty()).then(|| format!("fold: {}", parts.join(" · ")))
}

/// A domain that puts one count in the footer of every tool result. A source that cannot count reports zero and logs why.
pub trait FooterSource: Send + Sync {
    fn part(&self) -> FooterPart;

    fn count<'a>(
        &'a self,
        session: &'a SessionContext,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + 'a>>;
}
