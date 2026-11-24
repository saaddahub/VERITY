//! Probe: what drive health WMI reports on this machine, and how reliability
//! counters are reached: by plain enumeration, or only through each physical
//! disk's association (docs/14-storage.md).
//! `cargo run -p verity-probes --bin diskhealth` (reliability counters need admin).

use verity_sys::wmi::Wmi;

const NS: &str = r"ROOT\Microsoft\Windows\Storage";
const REL: [&str; 7] = [
    "DeviceId",
    "Temperature",
    "TemperatureMax",
    "Wear",
    "ReadErrorsTotal",
    "WriteErrorsTotal",
    "PowerOnHours",
];

fn main() {
    let w = Wmi::connect(NS).expect("connect");
    let props = [
        "__PATH",
        "DeviceId",
        "FriendlyName",
        "MediaType",
        "HealthStatus",
        "BusType",
    ];
    let disks = w
        .query(
            &format!("SELECT {} FROM MSFT_PhysicalDisk", props.join(", ")),
            &props,
        )
        .expect("physical disks");
    for d in &disks {
        println!(
            "disk {:?}",
            props
                .iter()
                .skip(1)
                .zip(d.iter().skip(1))
                .collect::<Vec<_>>()
        );
    }

    match w.query(
        &format!(
            "SELECT {} FROM MSFT_StorageReliabilityCounter",
            REL.join(", ")
        ),
        &REL,
    ) {
        Ok(rows) => println!("enumeration: {} rows {rows:?}", rows.len()),
        Err(e) => println!("enumeration: {e}"),
    }
    for d in &disks {
        let Some(path) = d[0].as_str() else { continue };
        let wql =
            format!("ASSOCIATORS OF {{{path}}} WHERE ResultClass = MSFT_StorageReliabilityCounter");
        match w.query(&wql, &REL) {
            Ok(rows) => println!("association of disk {:?}: {rows:?}", d[1]),
            Err(e) => println!("association of disk {:?}: {e}", d[1]),
        }
    }
}
