//! The Buwiz server, or with `seed` its seed tool
//! ([`buwiz_cloud::account::seed`]).

use std::process::ExitCode;

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("seed") {
        return Ok(buwiz_cloud::account::seed::cli(&args[1..]).await);
    }
    buwiz_cloud::run().await?;
    Ok(ExitCode::SUCCESS)
}
