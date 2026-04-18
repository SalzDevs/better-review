<div align="center">

<img src="demo/better-review.gif" alt="better-review demo" width="920" />

# better-review

**A review-first terminal UI for agent-generated code changes.**

Run your coding agent, inspect the resulting diff in a focused fullscreen TUI, accept/reject by file or hunk, and commit only what you approve.

[Demo](#demo) • [Features](#features) • [Quick Start](#quick-start) • [Safety Model](#safety-model) • [Architecture](#architecture) • [Development](#development)

</div>

---

## Demo

The project includes a reproducible end-to-end demo that shows the intended workflow:

1. Launch `better-review`
2. Open the composer (`Ctrl+O`) and run an `opencode` prompt
3. Review generated changes in file/hunk mode
4. Accept some changes, reject others
5. Commit accepted changes only

### Watch

- GIF preview: `demo/better-review.gif`
- MP4: `demo/better-review.mp4`

### Re-record the demo locally

```bash
vhs demo/better-review.tape
```

Demo sources used to generate the recording:

- Tape: `demo/better-review.tape`
- Runner: `demo/run-demo.sh`
- Mock agent CLI: `demo/mock-opencode.sh`
- Fixture repo: `demo/fixture/`

## Why better-review

Coding agents accelerate implementation, but they also make it easy to skip intentional review.

`better-review` adds a dedicated review surface between generation and commit. Instead of trusting raw output or manually juggling git commands, you can evaluate changes in one place and decide exactly what becomes commit-eligible.

## Features

- **Review-first flow**: compose prompt -> run `opencode` -> review -> commit
- **Session-scoped diffing**: isolate only changes produced during the current session
- **File + hunk decisions**: accept/reject at the granularity you need
- **Accepted-only commit path**: commit exactly what you approved
- **Workspace protection**: preserve unrelated dirty/staged work from before the run
- **Non-destructive reject semantics**: reject controls commit eligibility rather than nuking your worktree
- **Fullscreen terminal UX**: home screen, review panes, commit modal, model picker
- **Terminal safety guardrails**: alternate screen and scrollback purge during app lifecycle

## Quick Start

### Prerequisites

- Rust toolchain
- `git`
- `opencode` available on `PATH`
- A git repository to review

### Run from source

```bash
cargo run
```

### Build release binary

```bash
cargo build --release
```

### Install locally

```bash
cargo install --path .
```

## Usage

Start `better-review` in the repository you want to review:

```bash
better-review
```

During development:

```bash
cargo run
```

### Keybindings

| Key | Action |
| --- | --- |
| `Ctrl+O` | Open composer |
| `Enter` | Submit, enter review, or drill into hunks |
| `Esc` | Close modal, go back from hunks, or return home |
| `Tab` | Model picker / hunk cycling |
| `Ctrl+T` | Cycle model variant |
| `y` | Accept file or hunk |
| `x` | Reject file or hunk |
| `u` | Move file back to unreviewed |
| `c` | Open commit prompt |
| `Ctrl+C` | Quit |

## Architecture

- `src/app.rs`: TUI shell, event loop, screens, overlays, rendering
- `src/services/git.rs`: snapshotting, diff collection, hunk sync, commit safety
- `src/services/opencode.rs`: model loading and agent execution
- `src/services/parser.rs`: diff parsing logic
- `src/domain/`: session/diff/model domain structures
- `src/ui/styles.rs`: shared styling and palette

## Development

### Test suite

```bash
cargo test -- --nocapture
```

## FAQ

### Does this replace git?

No. `better-review` is a review surface and commit gate on top of your existing git workflow.

### Can I use it in a dirty repository?

Yes. The snapshot model is specifically designed to protect preexisting dirty/staged work.

### Why not just `git add -p`?

`git add -p` is powerful, but `better-review` is optimized for the agent workflow: compose prompt, inspect generated diff, decide quickly, commit accepted changes only.

