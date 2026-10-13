//! The server's one handle on the message cache: a [`Sink`]. `#208 persistent-cache`.
//!
//! The handle is always present, in every server state, whether or not a cache is open behind it,
//! so a read site never has to ask whether the cache exists before it asks whether it may fill
//! it. Every fill starts with [`Sink::begin`], and only under a [`FillGrant`]: a read-scope caller
//! has none (see [`super::grant`]), so it never reaches the cache at all.
//!
//! In this build nothing installs a cache, so the slot stays empty and every fill is refused.

use std::sync::{Arc, PoisonError, RwLock};

use super::grant::FillGrant;
use super::MessageCache;
use crate::model::ChannelId;

/// The message cache as the server sees it: an empty slot, or an open cache.
#[derive(Debug, Default)]
pub struct Sink {
    slot: RwLock<Option<Arc<MessageCache>>>,
}

/// One upstream read's permission to offer what it sees, taken by [`Sink::begin`] before the read
/// starts.
#[derive(Debug)]
pub struct Fill {
    grant: FillGrant,
    channel: ChannelId,
}

impl Fill {
    /// The grant the fill was begun under.
    #[must_use]
    pub fn grant(&self) -> FillGrant {
        self.grant
    }

    /// The channel being read.
    #[must_use]
    pub fn channel(&self) -> &ChannelId {
        &self.channel
    }
}

impl Sink {
    /// A sink with no cache behind it: every fill is refused.
    #[must_use]
    pub fn off() -> Self {
        Self::default()
    }

    /// Begin a fill of `channel` under `grant`, before its upstream read starts. `None` means
    /// nothing the read sees is offered: there is no cache to offer it to.
    #[must_use]
    pub fn begin(&self, grant: &FillGrant, channel: &ChannelId) -> Option<Fill> {
        self.installed().map(|_cache| Fill {
            grant: *grant,
            channel: channel.clone(),
        })
    }

    /// The cache in the slot, if any.
    fn installed(&self) -> Option<Arc<MessageCache>> {
        self.slot
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::super::grant::ServerFill;
    use super::*;
    use crate::auth::Scope;

    #[test]
    fn a_sink_that_is_off_begins_no_fill_under_any_grant() {
        let sink = Sink::off();
        let channel = ChannelId("1000000000000000001".to_owned());
        let caller = FillGrant::for_caller(Scope::Write).expect("write scope may fill");
        assert!(sink.begin(&caller, &channel).is_none());
        for kind in ServerFill::ALL {
            assert!(
                sink.begin(&FillGrant::server(kind), &channel).is_none(),
                "{kind:?}"
            );
        }
    }
}
