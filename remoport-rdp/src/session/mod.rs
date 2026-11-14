//! The active session: server updates out as frames, input in as commands.
//! The loop, the reactivation sequence and the region helpers follow
//! RustConn's client (`rustconn-core`, GPL-3.0-or-later); the clipboard's
//! handling is Remoport's.

mod commands;
mod output;
mod run;

pub(crate) use run::run;
