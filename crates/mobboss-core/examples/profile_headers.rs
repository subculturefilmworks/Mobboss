//! Read-only diagnostic: cargo run --release -p mobboss-core --example profile_headers -- <root>
use anyhow::{Context, Result};
use mobboss_core::{discover, mxf};
use std::{
    cell::RefCell,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};
#[derive(Default)]
struct Stats {
    bytes: usize,
    read: Duration,
    seek: Duration,
}
struct Timed {
    file: File,
    stats: Rc<RefCell<Stats>>,
}
impl Read for Timed {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let t = Instant::now();
        let r = self.file.read(b);
        let mut s = self.stats.borrow_mut();
        s.read += t.elapsed();
        if let Ok(n) = r {
            s.bytes += n;
        }
        r
    }
}
impl Seek for Timed {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        let t = Instant::now();
        let r = self.file.seek(p);
        self.stats.borrow_mut().seek += t.elapsed();
        r
    }
}
fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args_os().nth(1).context("supply scan root")?);
    let mut warnings = Vec::new();
    let paths = discover(&root, &mut warnings);
    let total = Instant::now();
    let stats = Rc::new(RefCell::new(Stats::default()));
    let mut opens = Duration::ZERO;
    let mut closes = Duration::ZERO;
    let mut good = 0;
    for path in &paths {
        let t = Instant::now();
        let f = File::open(path);
        opens += t.elapsed();
        let f = match f {
            Ok(f) => f,
            Err(e) => {
                warnings.push(e.to_string());
                continue;
            }
        };
        let mut r = Timed {
            file: f,
            stats: stats.clone(),
        };
        match mxf::parse(&mut r) {
            Ok(_) => good += 1,
            Err(e) => warnings.push(format!("{}: {e}", path.display())),
        };
        let t = Instant::now();
        drop(r);
        closes += t.elapsed();
    }
    let wall = total.elapsed();
    let s = stats.borrow();
    println!("{good}/{} headers; {} bytes; wall {:.3}s; opens {:.3}s; reads {:.3}s; seeks {:.3}s; closes {:.3}s; other {:.3}s",paths.len(),s.bytes,wall.as_secs_f64(),opens.as_secs_f64(),s.read.as_secs_f64(),s.seek.as_secs_f64(),closes.as_secs_f64(),wall.saturating_sub(opens+s.read+s.seek+closes).as_secs_f64());
    for w in warnings {
        eprintln!("warning: {w}");
    }
    Ok(())
}
