#[tokio::main]
async fn main() -> anyhow::Result<()> {
    buwiz_cloud::run().await
}
