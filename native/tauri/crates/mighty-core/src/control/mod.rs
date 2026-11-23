//! MC-9: scripts drive the app and hear what its panes do. Pure parts only; the app (src-tauri `control`) hosts them.
//!
//! - [`protocol`]: the local control API's wire — one JSON object per line, each request carrying the token, answered by one line
//!   (`ok` with a `result`, or an `error` with a `code`), and for `events.subscribe` a stream of `event` lines after it. Every line is
//!   capped ([`protocol::MAX_LINE_BYTES`]).
//! - [`events`]: a pane's state (idle, running, blocked, done, failed, stopped), the transitions hooks and webhooks follow, and the
//!   events the app's one bus carries (state, attention, a run's end with its cost). No event carries a prompt, an answer or a file.
//! - [`limit`]: the webhooks' debounce and rate limit, with the time given.
//! - [`hooks`]: the variables a status hook gets.
//! - [`cli`]: the `mighty` CLI's arguments read into a request, and its exit codes.
//! - [`text`]: terminal output as plain lines, and a pane's transcript as lines.

pub mod cli;
pub mod events;
pub mod hooks;
pub mod limit;
pub mod protocol;
pub mod text;
