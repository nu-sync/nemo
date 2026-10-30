use std::{
    io::{self, BufRead, Write},
    sync::atomic::AtomicBool,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[allow(dead_code)]
mod output {
    include!(env!("NEMO_OUTPUT_SOURCE"));

    pub fn baseline(text: &str) -> anyhow::Result<()> {
        use std::io::Write;
        let mut child = Command::new("/usr/bin/pbcopy")
            .stdin(Stdio::piped())
            .spawn()?;
        child.stdin.take().unwrap().write_all(text.as_bytes())?;
        anyhow::ensure!(child.wait()?.success(), "pbcopy failed");
        thread::sleep(std::time::Duration::from_millis(100));
        post_paste(true, &AtomicBool::new(false))
    }
}

fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        output::trusted() && output::can_post_events(),
        "Existing Accessibility permission required"
    );
    for line in io::stdin().lock().lines() {
        let line = line?;
        let (variant, text) = line
            .split_once('\t')
            .ok_or_else(|| anyhow::anyhow!("variant and fixture required"))?;
        anyhow::ensure!(
            output::frontmost_app() == "Nemo Delivery Check",
            "Scratch target lost foreground focus"
        );
        let unix_seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
        let started = Instant::now();
        let canceled = AtomicBool::new(variant == "canceled");
        let result = if variant == "baseline" {
            output::baseline(text)
        } else {
            output::deliver(text, variant != "copy", &canceled)
        };
        if variant == "canceled" {
            anyhow::ensure!(
                result.unwrap_err().to_string() == "Canceled",
                "Cancellation was not honored"
            );
        } else {
            result?;
        }
        println!(
            "{}",
            serde_json::json!({"start_unix_seconds": unix_seconds, "dispatch_ms": started.elapsed().as_secs_f64() * 1000.0})
        );
        io::stdout().flush()?;
    }
    Ok(())
}
