use mobboss_core::{CancellationToken, Index, ScanMode, ScanOptions, ScanPhase};
use std::{path::PathBuf, time::Instant};
fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).expect("media root"));
    let order = std::env::args().nth(2).unwrap_or_else(|| "1,4,8,16".into());
    for concurrency in order.split(',').map(str::parse::<usize>) {
        let concurrency = concurrency?;
        let path = std::env::temp_dir().join(format!(
            "mobboss-bench-{}-{concurrency}.sqlite",
            std::process::id()
        ));
        let mut index = Index::open_for_scan(&path, &root)?;
        let start = Instant::now();
        let mut first = None;
        let report = index.scan_with(
            &root,
            &ScanOptions {
                mode: ScanMode::Quick,
                concurrency,
            },
            &CancellationToken::default(),
            |p| {
                if matches!(p.phase, ScanPhase::Inventory) {
                    first = Some(start.elapsed().as_secs_f64());
                    println!(
                        "workers={concurrency} inventory={} named={} first={:.3}s",
                        p.files.len(),
                        p.files
                            .iter()
                            .filter(|f| f.metadata.project.as_deref() == Some("BD Demo Reel"))
                            .count(),
                        first.unwrap()
                    );
                }
            },
        )?;
        println!(
            "workers={concurrency} files={} headers={} enriched={:.3}s",
            report.files,
            report.headers_read,
            start.elapsed().as_secs_f64()
        );
        drop(index);
        std::fs::remove_file(path)?;
    }
    Ok(())
}
