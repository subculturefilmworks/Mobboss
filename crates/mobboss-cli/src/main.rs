use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use mobboss_core::{default_index, Index, ScanMode};
use std::path::PathBuf;
#[derive(Parser)]
#[command(version, about = "Mob Boss — read-only Avid media inventory")]
struct Args {
    #[arg(long, global = true)]
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
    Scan {
        path: PathBuf,
        #[arg(long, value_enum, default_value = "auto")]
        mode: Mode,
    },
    Projects,
    Files {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, conflicts_with = "csv")]
        json: bool,
        #[arg(long)]
        csv: bool,
    },
    Diff {
        path: PathBuf,
    },
}
fn main() -> Result<()> {
    let args = Args::parse();
    let path = args.index.map(Ok).unwrap_or_else(default_index)?;
    // Validate before opening SQLite so a rejected scan cannot create a database in media.
    if let Command::Scan { path: root, .. } | Command::Diff { path: root } = &args.command {
        let root = root.canonicalize()?;
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir()?.join(&path)
        };
        anyhow::ensure!(
            !absolute.starts_with(root),
            "index must be outside scanned media tree"
        );
    }
    let mut index = Index::open(&path)?;
    match args.command {
        Command::Scan { path, mode } => {
            let r = index.scan(
                &path,
                match mode {
                    Mode::Quick => ScanMode::Quick,
                    Mode::Auto => ScanMode::Auto,
                    Mode::Deep => ScanMode::Deep,
                },
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
