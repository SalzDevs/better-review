#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewConfig {
    pub require_publish_confirmation: bool,
    pub default_remote: String,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            require_publish_confirmation: true,
            default_remote: "origin".to_string(),
        }
    }
}
