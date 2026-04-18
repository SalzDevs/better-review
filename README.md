# better-review

`better-review` is a fullscreen terminal UI for reviewing agent-generated code changes before you commit them.

It runs `opencode`, captures only the changes created during the current session, lets you review diffs file by file or hunk by hunk, and commits only the changes you explicitly accept.

## Why it exists

Agent workflows are fast, but raw terminal output and immediate commits make review easy to skip.

`better-review` puts a review surface between generation and commit:

- inspect every changed file in one place
- accept or reject at the file or hunk level
- commit accepted changes only
- protect unrelated dirty or staged work already in your repo
- stay inside a fullscreen TUI without exposing previous terminal scrollback

## What it does

- launches into a dedicated home screen
- opens an inline composer with `Ctrl+O`
- runs `opencode` against the current repository
- snapshots the workspace before the run
- shows a session-scoped diff for review
- supports file-level and hunk-level accept/reject
- stages only accepted hunks when syncing review decisions
- blocks unsafe commit flows when the session started with unrelated staged changes

## Core workflow

1. Open the composer with `Ctrl+O`
2. Write the instruction you want to send to `opencode`
3. Review the generated diff in `better-review`
4. Accept with `y`, reject with `x`, or move a file back to unreviewed with `u`
5. Commit accepted changes with `c`

## Keybindings

- `Ctrl+O` open composer
- `Enter` submit prompt, enter review, or drill into hunks
- `Esc` close modal, move back from hunks, or return home from file review
- `Tab` open model picker or cycle hunks
- `Ctrl+T` cycle model variant in composer
- `y` accept file or hunk
- `x` reject file or hunk
- `u` move a file back to unreviewed
- `c` open commit prompt
- `Ctrl+C` quit

## Safety model

`better-review` is designed to avoid damaging unrelated work in progress.

- it snapshots the index and worktree before an agent run
- it collects only session-created changes for review
- it restores staged state safely when rejecting files
- it treats reject decisions as index/commit eligibility, not destructive worktree rewrites
- it prevents commits when the session started with unrelated staged changes

## Requirements

- Rust toolchain
- `opencode` available on your `PATH`
- Git repository workspace

## Run locally

```bash
cargo run
```

## Test

```bash
cargo test -- --nocapture
```

## Current status

The project already has solid core behavior:

- robust model parsing for `opencode models --verbose`
- session-safe diff isolation
- accepted-only commit flow
- async hunk syncing to avoid TUI freezes
- regression coverage around git/session behavior

The main remaining work is product polish: docs, additional UX refinements, and more launch-ready presentation.

## Launch tweet draft

> I built `better-review`, a fullscreen terminal UI for reviewing agent-generated code before committing it.
>
> It runs `opencode`, snapshots your repo, shows a session-only diff, and lets you accept/reject files or hunks before committing accepted changes only.
>
> Built in Rust with `ratatui`.
>
> If you're using coding agents but still want a real review step, this is for you.
