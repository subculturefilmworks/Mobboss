use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use mobboss_core::{default_index, CancellationToken, Index, ScanMode, ScanOptions};
use std::path::PathBuf;
#[derive(Parser)]
#[command(
    version,
    about = "Mob Boss — Avid media inventory and verified collection"
)]
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
#[derive(clap::Args)]
struct TransferArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    dest: Option<PathBuf>,
    /// Execute the displayed plan. Quit Media Composer first.
    #[arg(long)]
    yes: bool,
    /// Optional indexed paths; omit to use the entire project.
    #[arg(long = "file")]
    files: Vec<PathBuf>,
}
#[derive(Subcommand)]
enum Command {
    /// Preview a copy plan without changing media.
    Plan(TransferArgs),
    Move(TransferArgs),
    Copy(TransferArgs),
    Collect(TransferArgs),
    /// Preview undo; pass --yes to reverse verified transfers.
    Undo {
        log: PathBuf,
        #[arg(long)]
        yes: bool,
    },
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
    let cancel = CancellationToken::default();
    let interrupt = cancel.clone();
    ctrlc::set_handler(move || interrupt.cancel())?;
    let path = args.index.map(Ok).unwrap_or_else(default_index)?;
    let mut index = match &args.command {
        Command::Scan { path: root, .. } | Command::Diff { path: root } => {
            Index::open_for_scan(&path, root)?
        }
        _ => Index::open(&path)?,
    };
    let operation = match &args.command {
        Command::Move(_) => mobboss_core::plan::Operation::Move,
        Command::Collect(_) => mobboss_core::plan::Operation::Collect,
        _ => mobboss_core::plan::Operation::Copy,
    };
    let preview = matches!(&args.command, Command::Plan(_));
    match args.command {
        Command::Plan(a) | Command::Move(a) | Command::Copy(a) | Command::Collect(a) => {
            let plan = mobboss_core::ops::prepare(
                &index,
                operation,
                &a.project,
                &a.files,
                a.dest.as_deref(),
            )?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
            eprintln!("Quit Media Composer before moving media. Avid will rebuild its media databases on next launch.");
            if a.yes && !preview {
                let data = default_index()?.parent().unwrap().to_path_buf();
                let result = mobboss_core::ops::execute(&mut index, &plan, &data, &cancel, |p| {
                    eprintln!("{} / {}  {}", p.done, p.total, p.source.display())
                })?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                eprintln!("Dry run only. Use move/copy/collect --yes to execute.");
            }
        }
        Command::Undo { log, yes } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&mobboss_core::ops::preview_undo(&index, &log)?)?
            );
            eprintln!("Quit Media Composer before undo. Copies are deleted only if their checksum still matches.");
            if yes {
                let data = default_index()?.parent().unwrap().to_path_buf();
                let result = mobboss_core::ops::undo(&mut index, &log, &data, &cancel, |p| {
                    eprintln!("{} / {}", p.done, p.total)
                })?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                eprintln!("Dry run only. Pass --yes to undo.");
            }
        }
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
                &cancel,
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
