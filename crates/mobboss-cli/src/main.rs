use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use mobboss_core::{default_index, CancellationToken, Index, ScanMode, ScanOptions};
use std::path::PathBuf;
#[derive(Parser)]
#[command(
    version = env!("MOBBOSS_VERSION"),
    about = "Mob Boss — MXF media management for Avid editors"
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
    /// Execute the displayed plan. Quit Avid first.
    #[arg(long)]
    yes: bool,
    /// Emit the complete machine-readable plan/result instead of a readable summary.
    #[arg(long)]
    json: bool,
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
    /// Move media on each drive, then copy it to the required backup destination.
    #[command(visible_alias = "move-and-backup")]
    Collect(TransferArgs),
    /// Preview undo; pass --yes to reverse verified transfers.
    Undo {
        log: PathBuf,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
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
#[cfg(test)]
mod build_version;

fn main() -> Result<()> {
    if std::env::args_os().len() == 2
        && std::env::args_os()
            .nth(1)
            .is_some_and(|arg| arg == "--version" || arg == "-V")
    {
        println!("{}", env!("MOBBOSS_VERSION"));
        return Ok(());
    }
    let args = Args::parse();
    if let Command::Collect(a) = &args.command {
        anyhow::ensure!(a.dest.is_some(), "Move & Backup (collect) requires --dest");
    }
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
            if a.json {
                println!("{}", serde_json::to_string_pretty(&plan)?);
            } else {
                println!("{}", plan_summary(&plan));
            }
            eprintln!("Quit Avid before moving media. Avid will rebuild its media databases on next launch.");
            if a.yes && !preview {
                let data = default_index()?.parent().unwrap().to_path_buf();
                let result = mobboss_core::ops::execute(&mut index, &plan, &data, &cancel, |p| {
                    eprintln!(
                        "{} / {}  {}",
                        p.done,
                        p.total,
                        mobboss_core::paths::display(&p.source).display()
                    )
                })?;
                if a.json {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                } else {
                    println!("{}", outcome_summary(&result));
                }
            } else {
                eprintln!("Dry run only. Use move/copy/move-and-backup --yes to execute.");
            }
        }
        Command::Undo { log, yes, json } => {
            let preview = mobboss_core::ops::preview_undo(&index, &log)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&preview)?);
            } else {
                println!(
                    "Dry-run undo: {} actions in reverse order ({} moves, {} copies)",
                    preview.rows.len(),
                    preview.rows.iter().filter(|r| r.op == "move").count(),
                    preview.rows.iter().filter(|r| r.op == "copy").count()
                );
                println!("{}", drive_summary(&preview.drives));
                for warning in preview.warnings {
                    println!("Warning: {warning}");
                }
            }
            eprintln!("Quit Avid before undo. Copies are deleted only if their checksum still matches.");
            if yes {
                let data = default_index()?.parent().unwrap().to_path_buf();
                let result = mobboss_core::ops::undo(&mut index, &log, &data, &cancel, |p| {
                    eprintln!("{} / {}", p.done, p.total)
                })?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                } else {
                    println!("{}", outcome_summary(&result));
                }
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
            let c = &r.reconciliation;
            println!("{} media files · {} Avid database files (.pmr/.mdb) · {} other files · {} folders — total {} bytes", c.media_files, c.database_files.len(), c.other_files, c.folders, c.total_bytes);
            for db in &c.database_files {
                println!(
                    "{}: {}; {}; modified ns {}",
                    mobboss_core::paths::display(&db.path).display(),
                    db.description,
                    display_bytes(db.size),
                    db.modified_ns
                );
                if let (Some(missing), Some(extra)) = (db.pmr_missing, db.pmr_extra) {
                    println!(
                        "PMR vs MXF: {}",
                        if missing == 0 && extra == 0 {
                            "agree".into()
                        } else {
                            format!("{missing} missing from disk / {extra} extra on disk")
                        }
                    );
                }
            }
            for name in &c.other_names {
                println!("Other: {}", mobboss_core::paths::display(name).display());
            }
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
                        mobboss_core::paths::display(&f.path)
                            .to_string_lossy()
                            .into_owned(),
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
                        f.metadata.project.as_deref().unwrap_or("Unknown project"),
                        f.metadata.codec.as_deref().unwrap_or("Unknown"),
                        mobboss_core::paths::display(&f.path).display()
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

fn display_bytes(value: u64) -> String {
    format!("{} ({value} bytes)", mobboss_core::format_size(value))
}
fn drive_summary(drives: &[mobboss_core::plan::DriveSummary]) -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for drive in drives {
        let _ = writeln!(
            text,
            "{}\n  {} files · {}\n  Free: {}\n  Required with margin: {}{}",
            mobboss_core::paths::display(&drive.root).display(),
            drive.files,
            display_bytes(drive.bytes),
            display_bytes(drive.available),
            display_bytes(drive.required),
            if drive.available < drive.required {
                " — BLOCKED: insufficient space"
            } else {
                ""
            }
        );
    }
    text
}
fn plan_summary(plan: &mobboss_core::plan::Plan) -> String {
    use std::fmt::Write;
    let mut text = format!("Dry-run plan: {}\n", plan.project);
    let steps: std::collections::BTreeSet<_> = plan.actions.iter().map(|a| a.step).collect();
    for step in steps {
        let actions: Vec<_> = plan.actions.iter().filter(|a| a.step == step).collect();
        let name = if actions[0].op == mobboss_core::plan::Operation::Move {
            "Move"
        } else {
            "Copy"
        };
        let _ = writeln!(
            text,
            "Step {}: {name} {} files ({} duplicate skips)",
            step + 1,
            actions.len(),
            actions.iter().filter(|a| a.duplicate).count()
        );
    }
    text.push_str(&drive_summary(&plan.drives));
    for warning in &plan.warnings {
        let _ = writeln!(text, "Warning: {warning}");
    }
    if plan.conflicts.is_empty() {
        text.push_str("Conflicts: none\n");
    }
    for conflict in &plan.conflicts {
        let _ = writeln!(text, "Conflict: {conflict}");
    }
    text
}
fn outcome_summary(result: &mobboss_core::ops::Outcome) -> String {
    let mut text = format!(
        "{} completed · {} skipped{}\n",
        result.completed,
        result.skipped,
        if result.cancelled {
            " · cancelled"
        } else {
            ""
        }
    );
    for warning in &result.warnings {
        text.push_str(&format!("Warning: {warning}\n"));
    }
    for folder in &result.touched {
        text.push_str(&format!(
            "Touched: {}\n",
            mobboss_core::paths::display(folder).display()
        ));
    }
    for log in &result.logs {
        text.push_str(&format!(
            "Log: {}\n",
            mobboss_core::paths::display(log).display()
        ));
    }
    text
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_is_opt_in_for_transfers_and_undo() {
        let args = Args::try_parse_from(["mobboss", "copy", "--project", "Film", "--dest", "/tmp"])
            .unwrap();
        assert!(matches!(
            args.command,
            Command::Copy(TransferArgs { json: false, .. })
        ));
        let args = Args::try_parse_from([
            "mobboss",
            "plan",
            "--project",
            "Film",
            "--dest",
            "/tmp",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            args.command,
            Command::Plan(TransferArgs { json: true, .. })
        ));
        assert!(Args::try_parse_from(["mobboss", "undo", "log.csv", "--json"]).is_ok());
    }
    #[test]
    fn move_and_backup_alias_keeps_collect_compatible() {
        for name in ["collect", "move-and-backup"] {
            let args =
                Args::try_parse_from(["mobboss", name, "--project", "Film", "--dest", "/backup"])
                    .unwrap();
            assert!(matches!(args.command, Command::Collect(_)));
        }
    }
    #[test]
    fn readable_summary_includes_space_and_refusal() {
        let summary = drive_summary(&[mobboss_core::plan::DriveSummary {
            root: "/shuttle".into(),
            files: 3,
            bytes: 10,
            required: 20,
            available: 5,
        }]);
        for expected in [
            "/shuttle",
            "3 files",
            "10 bytes",
            "Free:",
            "Required with margin:",
            "BLOCKED",
        ] {
            assert!(summary.contains(expected));
        }
    }
}
