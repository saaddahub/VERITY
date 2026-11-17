//! Probe: what the library scan finds on this machine.
//! `cargo run -p verity-probes --bin games`

fn main() {
    let roots = verity_session::real_roots();
    println!("steam root: {:?}", roots.steam);
    println!("epic manifests: {:?}", roots.epic_manifests);
    println!("gog entries: {}", roots.gog.len());
    println!("volumes: {:?}", roots.volumes);
    let t = std::time::Instant::now();
    let games = verity_session::library::scan(&verity_session::StdFs, &roots);
    println!("scan took {:?}", t.elapsed());
    for g in games {
        println!(
            "{:<6} {:<10} {:<28} {}",
            g.source.as_str(),
            g.launcher_id.unwrap_or_default(),
            g.name,
            g.exe_path.display()
        );
    }
}
