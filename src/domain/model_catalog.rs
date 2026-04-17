#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub provider: String,
    pub name: String,
    pub variants: Vec<String>,
}
