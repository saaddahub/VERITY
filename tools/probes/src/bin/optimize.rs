//! Probe: how Optimize Drives is called through WMI (docs/14-storage.md).
//! Finds the physical disk behind a volume (volume -> partition -> disk number)
//! and runs `MSFT_Volume.Optimize` with ReTrim for an SSD, Defrag for an HDD.
//! Needs admin: `cargo run -p verity-probes --bin optimize -- C`

use verity_sys::wmi::Wmi;

const NS: &str = r"ROOT\Microsoft\Windows\Storage";

fn main() {
    let letter = std::env::args().nth(1).unwrap_or_else(|| "C".into());
    let w = Wmi::connect(NS).expect("connect");
    let vols = w
        .query(
            "SELECT __RELPATH, DriveLetter FROM MSFT_Volume WHERE DriveType = 3",
            &["__RELPATH", "DriveLetter"],
        )
        .expect("volumes");
    let code = letter.chars().next().expect("letter") as u64;
    let Some(rel) = vols
        .iter()
        .find(|v| v[1].as_u64() == Some(code))
        .and_then(|v| v[0].as_str().map(String::from))
    else {
        println!("no volume {letter}:");
        return;
    };
    let parts = w
        .query(
            &format!("ASSOCIATORS OF {{{rel}}} WHERE ResultClass = MSFT_Partition"),
            &["DiskNumber"],
        )
        .expect("partitions");
    println!("partitions of {letter}: {parts:?}");
    let disk = parts.first().and_then(|p| p[0].as_u64());
    let media = disk.and_then(|n| {
        w.query(
            &format!("SELECT MediaType FROM MSFT_PhysicalDisk WHERE DeviceId = '{n}'"),
            &["MediaType"],
        )
        .ok()?
        .first()?
        .first()?
        .as_u64()
    });
    println!("disk {disk:?} media {media:?}");
    let arg = if media == Some(3) { "Defrag" } else { "ReTrim" };
    let t = std::time::Instant::now();
    let r = w.call("MSFT_Volume", &rel, "Optimize", &[(arg, true)]);
    println!(
        "Optimize({arg}) -> {r:?} in {:.1} s",
        t.elapsed().as_secs_f64()
    );
}
