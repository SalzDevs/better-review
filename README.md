<div align="center">

<img src="demo/better-review.gif" alt="better-review demo" width="920" />

# better-review

**A review-first terminal UI for agent-generated code changes.**

Run your coding agent, inspect the resulting diff in a focused fullscreen TUI, accept/reject by file or hunk, and commit only what you approve.

[Demo](#demo) • [Features](#features) • [Installation](#installation) • [Quick Start](#quick-start) • [Architecture](#architecture) • [Development](#development)

</div>

---

## Demo

The project includes a reproducible end-to-end demo that shows the intended workflow:

1. Launch `better-review`
2. Make changes in the repo with your agent or editor
3. Re-open `better-review` and inspect generated changes in file/hunk mode
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
- Fixture repo: `demo/fixture/`

## Why better-review

Coding agents accelerate implementation, but they also make it easy to skip intentional review.

`better-review` adds a dedicated review surface between generation and commit. Instead of trusting raw output or manually juggling git commands, you can evaluate changes in one place and decide exactly what becomes commit-eligible.

## Features

- **Review-first flow**: run your agent however you like -> open review -> commit accepted changes
- **Workspace diffing**: inspect current repository changes in one focused surface
- **File + hunk decisions**: accept/reject at the granularity you need
- **Accepted-only commit path**: commit exactly what you approved
- **Workspace protection**: preserve unrelated dirty/staged work from before the run
- **Non-destructive reject semantics**: reject controls commit eligibility rather than nuking your worktree
- **Pure review workflow**: run your coding agent however you like, then use `better-review` to inspect and gate the result
- **Fullscreen terminal UX**: home screen, review panes, and commit modal
- **Terminal safety guardrails**: alternate screen and scrollback purge during app lifecycle

## Installation

### One-command install (no Rust required)

```bash
curl -fsSL https://raw.githubusercontent.com/Ricardo-Ceia/better-review/main/install.sh | sh
```

### Install a specific release

```bash
BETTER_REVIEW_VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/Ricardo-Ceia/better-review/main/install.sh | sh
```

### Custom install location

```bash
BETTER_REVIEW_BIN_DIR="$HOME/.local/bin" curl -fsSL https://raw.githubusercontent.com/Ricardo-Ceia/better-review/main/install.sh | sh
```

Environment variables:

- `BETTER_REVIEW_VERSION`: release tag (defaults to `latest`)
- `BETTER_REVIEW_REPO`: alternate `owner/repo` for forks
- `BETTER_REVIEW_BIN_DIR`: exact destination directory for the binary
- `BETTER_REVIEW_INSTALL_PREFIX`: installs to `<prefix>/bin` when `BETTER_REVIEW_BIN_DIR` is unset

## Quick Start

### Prerequisites

- `git`
- A git repository with changes to review
- Optional: your preferred coding agent (`opencode`, Claude Code, etc.)

### Run the installed binary

```bash
better-review
```

### Run from source (Rust required)

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
| `Enter` | Enter review or drill into hunks |
| `Esc` | Close modal, go back from hunks, or return home |
| `Tab` | Cycle hunks |
| `y` | Accept file or hunk |
| `x` | Reject file or hunk |
| `u` | Move file back to unreviewed |
| `c` | Open commit prompt |
| `Ctrl+C` | Quit |

## Architecture

- `src/app.rs`: TUI shell, event loop, screens, overlays, rendering
- `src/services/git.rs`: diff collection, hunk sync, commit safety
- `src/services/parser.rs`: diff parsing logic
- `src/domain/`: diff domain structures
- `src/ui/styles.rs`: shared styling and palette

## Development

### Test suite

```bash
cargo test -- --nocapture
```

### Release process

- Push a version tag like `v0.1.0` to trigger `.github/workflows/release.yml`
- The workflow builds Linux + macOS binaries, packages `tar.gz` archives, and uploads `.sha256` checksums
- `install.sh` downloads those release artifacts (`latest` by default)

```bash
git tag v0.1.0
git push origin v0.1.0
```

## FAQ

### Does this replace git?

No. `better-review` is a review surface and commit gate on top of your existing git workflow.

### Can I use it in a dirty repository?

Yes. `better-review` is designed to preserve preexisting dirty/staged work and only gate what is commit-eligible.

### Why not just `git add -p`?

`git add -p` is powerful, but `better-review` is optimized for the agent workflow: run your coding agent, return to a focused review surface, decide quickly, and commit accepted changes only.
