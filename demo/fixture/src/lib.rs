pub mod audit;
pub mod config;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewItem {
    pub path: String,
    pub accepted: bool,
}

pub fn review_summary(items: &[ReviewItem]) -> String {
    let accepted = items.iter().filter(|item| item.accepted).count();
    format!("{accepted}/{} changes accepted", items.len())
}

pub fn publish_branch_name(user: &str, ticket: u32) -> String {
    format!("review/{user}-{ticket}")
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

        assert_eq!(review_summary(&items), "1/2 changes accepted");
    }
}
