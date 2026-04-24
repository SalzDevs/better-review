pub fn risk_label(score: u8) -> &'static str {
    match score {
        0..=39 => "low",
        40..=74 => "medium",
        _ => "high",
    }
}

pub fn audit_message(path: &str, score: u8) -> String {
    format!("{path}: {} risk", risk_label(score))
}
