#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmpdir="$(mktemp -d)"

cp -R "${repo_root}/demo/fixture/." "${tmpdir}/"
git -C "${tmpdir}" init -q
git -C "${tmpdir}" config user.name "better-review demo"
git -C "${tmpdir}" config user.email "demo@example.com"
git -C "${tmpdir}" add .
git -C "${tmpdir}" commit -qm "base review service"

cat > "${tmpdir}/src/lib.rs" <<'RS'
pub mod audit;
pub mod config;
pub mod debug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewItem {
    pub path: String,
    pub accepted: bool,
}

pub fn review_summary(items: &[ReviewItem]) -> String {
    let total = items.len();
    let accepted = items.iter().filter(|item| item.accepted).count();
    let rejected = total.saturating_sub(accepted);

    format!("{accepted} accepted, {rejected} left to review")
}

pub fn publish_branch_name(user: &str, ticket: u32) -> String {
    let slug = user.trim().to_lowercase().replace(' ', "-");
    format!("review/{slug}-{ticket}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_summary_counts_accepted_items() {
        let items = vec![
            ReviewItem {
                path: "src/lib.rs".to_string(),
                accepted: true,
            },
            ReviewItem {
                path: "src/debug.rs".to_string(),
                accepted: false,
            },
        ];

        assert_eq!(review_summary(&items), "1 accepted, 1 left to review");
    }
}
RS

cat > "${tmpdir}/src/config.rs" <<'RS'
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewConfig {
    pub require_publish_confirmation: bool,
    pub default_remote: String,
    pub auto_refresh: bool,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            require_publish_confirmation: true,
            default_remote: "origin".to_string(),
            auto_refresh: true,
        }
    }
}
RS

cat > "${tmpdir}/src/debug.rs" <<'RS'
pub fn dump_review_payload(payload: &str) {
    println!("DEBUG review payload: {payload}");
}

pub fn disable_publish_guard_for_demo() -> bool {
    true
}
RS

printf '%s\n' "${tmpdir}"
