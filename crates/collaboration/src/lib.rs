//! The team collaboration context: how a desktop talks to the PocketBase
//! team backend.
//!
//! - [`contract`] is the wire contract of the protected command route
//!   (`infra/pocketbase/pb_hooks/collab_commands.pb.js`) and of the
//!   provisional collections (`infra/pocketbase/pb_migrations`).
//! - [`PocketBaseClient`] authenticates and calls the REST API.
//! - [`ConnectionProblem`] says in plain words why the server could not be
//!   reached.
//! - [`ServerTrust`] is the one TLS setup for `https://` servers, and
//!   [`ServerAddress`] holds the rules for the address typed at sign-in.
//! - [`RealtimeConnection`] receives change notifications over PocketBase's
//!   server-sent events.
//! - [`Replica`] keeps a local copy consistent: it applies changes in
//!   change-log sequence order, ignores duplicates, and catches up from its
//!   checkpoint after a disconnect or a gap.
//!
//! The collections are deliberately generic and provisional; the catalog
//! model (kicad-parts-fef6, kicad-parts-2254) will adopt these mechanics.

mod address;
mod client;
mod connection;
pub mod contract;
mod realtime;
mod replica;
mod tls;

pub use address::{AddressRefusal, ServerAddress};
pub use client::{HttpReply, PocketBaseClient};
pub use connection::ConnectionProblem;
pub use contract::{
    ChangeEntry, CommandOutcome, CommandReply, CorrelationId, EditRecord, RecordSnapshot,
};
pub use realtime::{RealtimeConnection, RealtimeEvent};
pub use replica::{ApplyResult, Replica, ReplicaRecord};
pub use tls::ServerTrust;

use std::fmt;

/// Why a collaboration call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollabError {
    /// The server could not be reached or the connection broke.
    Transport(ConnectionProblem),
    /// 401: not signed in, or the token is not valid for this route.
    Unauthorized(String),
    /// 403: signed in, but the rules forbid it.
    Forbidden(String),
    /// Any other unexpected HTTP status.
    Http { status: u16, message: String },
    /// The server answered something this client does not understand.
    Protocol(String),
    /// The address may not receive a password: plain `http://` to another
    /// computer. Nothing was sent.
    Address(AddressRefusal),
}

impl fmt::Display for CollabError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(problem) => problem.fmt(formatter),
            Self::Unauthorized(message) => write!(formatter, "not signed in: {message}"),
            Self::Forbidden(message) => write!(formatter, "not allowed: {message}"),
            Self::Http { status, message } => {
                write!(formatter, "the team server answered {status}: {message}")
            }
            Self::Protocol(message) => {
                write!(
                    formatter,
                    "unexpected answer from the team server: {message}"
                )
            }
            Self::Address(refusal) => refusal.fmt(formatter),
        }
    }
}

impl std::error::Error for CollabError {}

pub type CollabResult<T> = Result<T, CollabError>;
