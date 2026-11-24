//! Probe: how fast and how complete the Storage scan is on this machine,
//! as the current user (docs/14-storage.md, "Scanning").
//! `cargo run --release -p verity-probes --bin storage [-- C:\ [threads]]`

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use verity_storage::{scan, RealFs, ROOT};

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().unwrap_or_else(|| r"C:\".into()));
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let threads: usize = args.next().and_then(|t| t.parse().ok()).unwrap_or(cores);
    let drive = verity_sys::files::drives()
        .unwrap_or_default()
        .into_iter()
        .find(|d| d.root == root);

    let t = std::time::Instant::now();
    let tree = scan(
        &RealFs,
        &root,
        threads,
        &AtomicBool::new(false),
        &mut |_| {},
    );
    let secs = t.elapsed().as_secs_f64();
    let r = tree.node(ROOT).expect("root");
    let gb = |b: u64| b as f64 / 1e9;
    println!(
        "root {} with {threads} threads: {secs:.1} s",
        root.display()
    );
    println!(
        "nodes {}, files {}, unreadable folders {}",
        tree.len(),
        r.files,
        tree.unreadable
    );
    println!("counted {:.2} GB", gb(r.total));
    if let Some(d) = drive {
        let used = d.total - d.free;
        println!(
            "drive used {:.2} GB; counted is {:.1}% of used",
            gb(used),
            r.total as f64 / used as f64 * 100.0
        );
    }
    println!("largest top-level:");
    for c in tree.children(ROOT).into_iter().take(8) {
        let n = tree.node(c).expect("child");
        println!("  {:>8.2} GB  {}", gb(n.total), tree.name(c));
    }
}
