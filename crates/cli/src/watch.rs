use anyhow::{bail, Result};
use std::future::Future;

/// Run a render function in a loop, clearing the screen between iterations.
/// Exits cleanly on Ctrl+C.
pub async fn run_loop<F, Fut>(interval_secs: u64, json: bool, render: F) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    if json {
        bail!("--watch and --json are incompatible. JSON consumers should poll directly.");
    }

    let interval = std::time::Duration::from_secs(interval_secs.max(1));
    let term = console::Term::stdout();

    loop {
        term.clear_screen()?;
        println!(
            "Last updated: {} (every {}s) \u{2014} Ctrl+C to exit\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            interval_secs,
        );

        if let Err(e) = render().await {
            eprintln!("Error: {e:#}");
        }

        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = tokio::signal::ctrl_c() => {
                term.clear_line()?;
                return Ok(());
            }
        }
    }
}
