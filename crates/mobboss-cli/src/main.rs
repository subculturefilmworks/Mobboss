use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use mobboss_core::{default_index, CancellationToken, Index, ScanMode, ScanOptions};
use std::path::PathBuf;
#[derive(Parser)]
#[command(version, about = "Mob Boss — read-only Avid media inventory")]
struct Args {
    #[arg(
        long,
        global = true,
        help = "SQLite index location, outside media folders"
    )]
    index: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    Quick,
    Auto,
    Deep,
}
#[derive(Subcommand)]
enum Command {
    /// Scan a folder recursively without changing media.
    Scan {
        /// Folder or mounted drive to inventory.
        path: PathBuf,
        #[arg(
            long,
            value_enum,
            default_value = "auto",
            help = "Quick uses cache/PMR, auto adapts per folder, deep verifies every header"
        )]
        mode: Mode,
        /// Maximum simultaneous header reads (1–64).
        #[arg(long, default_value_t = 8)]
        concurrency: usize,
    },
    /// List indexed projects and file counts.
    Projects,
    /// List or export indexed media.
    Files {
        #[arg(long)]
        /// Restrict results to this project name.
        project: Option<String>,
        #[arg(long, conflicts_with = "csv")]
        /// Export complete metadata as JSON.
        json: bool,
        #[arg(long)]
        /// Export table rows as CSV.
        csv: bool,
    },
    /// Compare disk with PMR or the prior index and refresh the index.
    Diff {
        /// Folder to compare.
        path: PathBuf,
    },
}
fn main() -> Result<()> {
    let args = Args::parse();
    let path = args.index.map(Ok).unwrap_or_else(default_index)?;
    let mut index = match &args.command {
        Command::Scan { path: root, .. } | Command::Diff { path: root } => {
            Index::open_for_scan(&path, root)?
        }
        _ => Index::open(&path)?,
    };
    match args.command {
        Command::Scan {
            path,
            mode,
            concurrency,
        } => {
            let r = index.scan_with(
                &path,
                &ScanOptions {
                    concurrency,
                    mode: match mode {
                        Mode::Quick => ScanMode::Quick,
                        Mode::Auto => ScanMode::Auto,
                        Mode::Deep => ScanMode::Deep,
                    },
                },
                &CancellationToken::default(),
                |_| {},
            )?;
            for w in &r.warnings {
                eprintln!("warning: {w}");
            }
            println!(
                "{} MXFs; {} headers read; {} cached; {:.3}s",
                r.files, r.headers_read, r.cached, r.elapsed_seconds
            );
            for (p, n) in r.projects {
                println!("{p}: {n}");
            }
            println!(
                "Diff: {} missing, {} unlisted, {} changed",
                r.diff.missing.len(),
                r.diff.unlisted.len(),
                r.diff.changed.len()
            );
        }
        Command::Projects => println!("{}", serde_json::to_string_pretty(&index.projects()?)?),
        Command::Files { project, json, csv } => {
            let files = index.files(project.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&files)?)
            } else if csv {
                let mut w = csv::Writer::from_writer(std::io::stdout());
                w.write_record([
                    "path",
                    "project",
                    "clip",
                    "kind",
                    "codec",
                    "duration",
                    "edit_rate",
                    "size",
                ])?;
                for f in files {
                    let m = f.metadata;
                    w.write_record([
                        f.path.to_string_lossy().into_owned(),
                        m.project.unwrap_or_default(),
                        m.clip_name.unwrap_or_default(),
                        m.track_kind.unwrap_or_default(),
                        m.codec.unwrap_or_default(),
                        m.duration.map(|v| v.to_string()).unwrap_or_default(),
                        m.edit_rate
                            .map(|r| format!("{}/{}", r.numerator, r.denominator))
                            .unwrap_or_default(),
                        f.size.to_string(),
                    ])?;
                }
                w.flush()?;
            } else {
                for f in files {
                    println!(
                        "{}\t{}\t{}",
                        f.metadata.project.as_deref().unwrap_or("Unknown"),
                        f.metadata.codec.as_deref().unwrap_or("Unknown"),
                        f.path.display()
                    );
                }
            }
        }
        Command::Diff { path } => {
            let r = index.scan(&path, ScanMode::Quick)?;
            println!("{}", serde_json::to_string_pretty(&r.diff)?);
            for w in r.warnings {
                eprintln!("warning: {w}");
            }
        }
    }
    Ok(())
}
