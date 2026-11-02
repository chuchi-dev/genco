# Agent Instructions

Instructions for anyone (human or coding agent) changing this repository.

- Keep it simple. Build the smallest version that solves the actual
  request, follow nearby patterns, no abstractions for theoretical cases.
  Every code path must be reachable today.
- Logic goes into the Rust binary. `scripts/` is only for building and
  installing.
- Rust is formatted with `cargo fmt` (hard tabs, max width 80). The webui
  uses prettier (tabs, single quotes).
- The webui looks like Windows 95: use the components in
  `webui/src/components/win` (Window, Dialog, Form, Button, Tabs, Tree, Menu; a Dialog is `resizable` only when it holds a list, log or editor) and the
  variables in `webui/src/styles/_win.scss`, don't style one-offs.
- Ask and confirm with `message.ask` / `message.confirm` (Win95 boxes),
  never the browser's prompt() or confirm(). A `Form`'s onsubmit throws
  on an error, the form shows it; `done="Saved"` flashes after success.
- Text styles that repeat (`.error`, `.hint`, `.status`, `pre.code`,
  fieldsets, `.row-buttons`) and colors (`$error`, `$ok`, `$hint`,
  `$terminal-*`) are global in `app.scss` / `_win.scss`, don't redefine
  them in components. Loads that run again use `newest()` from lib/api.
- Rust helpers for commands, timestamps, tokens and file modes are in
  `util.rs`, job logs and locks of deploys and backups in `job.rs`.
- Every change through the API lands in the activity log
  (`activity.rs`): a new route gets a line in `ACTIONS` with what it
  does; request body fields named there are logged, so never put secrets
  under those names.
- Use blank lines to separate logical steps inside functions.
- Tests are small, offline unit tests next to the code
  (`#[cfg(test)] mod tests`).
- The agent's instructions (`server/src/agent/instructions.md`) are read
  on every request: a few lines on when and how, not implementation.
- Commits: a short imperative subject, then a body that says what changed
  and why.
- The plan and milestones live in `docs/PLAN.md`, update the checkboxes
  when something is done.

## Layout

- `server/`: the `genco` binary (axum, rusqlite, soe-llms). The built webui
  is embedded, so a release is a single file.
- `webui/`: Svelte 5 with the chuchi router, built into `webui/dist`.
- `scripts/build.sh` builds both, `scripts/install.sh` sets up a server.
