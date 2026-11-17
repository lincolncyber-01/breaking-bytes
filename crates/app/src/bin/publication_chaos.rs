//! Binário de caos restrito ao recurso test-support e a endpoints Graph em loopback.
fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(stories_app::publication::chaos_harness::run_once())
}
