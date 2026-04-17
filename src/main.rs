mod app;
mod domain;
mod services;
mod ui;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    app::run().await
}
