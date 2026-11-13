//! Filing and the relay, the Gatekeeper's side: device keys, tickets,
//! sealed envelopes, submissions, receipts and the trust list, and the link
//! to the Sender. The pipe is [`crate::relay`] and the Sender is
//! [`crate::sender`]. See `docs/relay.md`.

pub mod devices;
pub mod envelope;
pub mod gate;
pub mod jws;
pub mod link;
pub mod submissions;
pub mod tickets;
pub mod trust;
