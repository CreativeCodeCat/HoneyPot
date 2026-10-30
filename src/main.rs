#[tokio::main]
async fn main() {
    honeypot_bot::core::gateway::run().await;
}
