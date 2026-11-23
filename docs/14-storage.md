# 14 — Storage (M12)

Shows what fills each drive, frees space safely, and reports drive health and
maintenance.

Items marked **(verify)** must be confirmed with a probe before being relied on.

## Research summary

- **WizTree** reads the NTFS Master File Table directly. That needs
  administrator rights and works on local NTFS volumes only; it scans a 256 GB
  SSD in about 1.4 s and a 2 TB HDD in about 7.5 s. Its duplicate finder
  compares names, sizes and dates, not contents.
- **WinDirStat** and similar tools walk folders through the Win32 API: slower
  (tens of seconds to minutes) but they need no rights beyond the user's own.
- **Windows' own cleanup** (Storage Sense, Disk Cleanup) already knows how to
  remove Windows Update leftovers, Delivery Optimization files, a previous
  Windows installation, thumbnails and temporary files safely.
- **Drive health**: `MSFT_StorageReliabilityCounter` gives temperature, wear
  (percent of rated life used), read and write errors and power-on hours; some
  drives report wear as 0, so a 0 is shown as "not reported". Needs admin.
- **Optimize Drives** (TRIM for SSDs, defragmentation for HDDs) is already
  scheduled weekly by Windows; `MSFT_Volume.Optimize` runs it on demand.

## The key design rule: scanning and deleting happen in the app

The scan and every deletion run in the desktop app, under the user's own
Windows account, never in the service:

- A SYSTEM service that lists files for a standard user would reveal file names
  the user is not allowed to see. That would break the privilege boundary.
- `CLAUDE.md` forbids the service writing to a path supplied by a client.
- The user's own account means the user can only delete what they could
  already delete in Explorer.

The service contributes only what needs admin rights and contains no file
names: drive health counters and the Optimize Drives call.

## Scanning

- v1: parallel folder walk with `GetFileInformationByHandleEx` and
  `FileIdBothDirectoryInfo` (names, sizes and IDs in batches), falling back to
  `FindFirstFileExW` with `FindExInfoBasic` and `FIND_FIRST_EX_LARGE_FETCH`.
  Pick by a probe on the dev laptop **(verify)** speed.
- Sizes are allocation sizes (space actually used). Hard links count once (file
  ID set). Junctions and symlinks are not followed. OneDrive "online-only"
  files count as zero local space and are marked. Folders the user cannot read
  are listed as "not readable", never retried as admin.
- Progress and cancel; a full scan of the system drive targets under 60 s on
  the dev laptop's SSD.
- Results stay in the app's memory. File names are never written to Verity's
  database or logs (`09-safety.md` §7).
- Later (not v1): an MFT fast path needs an elevated helper and per-user
  filtering. It is noted for M8-style depth, not promised here.

## What the user sees

| View | Content |
|---|---|
| Drives | Each drive: used, free, health, last optimized |
| Folders | Tree sorted by size, with share of the drive |
| Largest files | Top 200 files over 100 MB |
| File types | Space by extension group (video, images, games, installers, archives, other) |
| Duplicates | Same size, then confirmed by SHA-256 of the contents (`sha2` is already a dependency); never by name alone |
| Clean-up suggestions | See below |

### Clean-up suggestions

| Suggestion | Measured how | What Verity does |
|---|---|---|
| Recycle Bin | `SHQueryRecycleBinW` | Empties it with `SHEmptyRecycleBinW`, after confirmation |
| Your temp folder | `%TEMP%` files older than 7 days and not in use | Moves them to the Recycle Bin, after confirmation |
| Old downloads | Downloads folder files older than 90 days | Lists them; the user ticks what goes |
| Installers | `.exe`, `.msi`, `.iso` in Downloads | Lists them; the user ticks what goes |
| Duplicates | As above | Keeps the newest copy ticked off by default; the user decides |
| Windows leftovers (update cleanup, Delivery Optimization files, previous Windows, thumbnails, crash dumps) | Size where readable, otherwise "Windows can tell you" | Opens Windows' own Storage Sense (`ms-settings:storagesense`). Verity does not delete system files itself |
| Games | Sizes from the game library | Shows which games take most space; uninstalling is the launcher's job |

## Deleting

- Only items the user ticked, after a confirmation that lists them and their
  total size.
- Moved to the Recycle Bin with `IFileOperation` and `FOFX_RECYCLEONDELETE`, so
  Explorer's rules apply (files in use, permissions) and every deletion can be
  restored. The result screen links to the Recycle Bin.
- When an item cannot go to the Recycle Bin (too large, a removable drive),
  Verity says so and offers permanent deletion behind a second confirmation
  that says it cannot be undone (owner-approved, 2026-11-23).
- Never offered, never deleted: anything under the Windows folder, Program
  Files, ProgramData, other users' profiles, files with the System attribute,
  Verity's own data, and anything the scan marked as in use.

## Honesty

The screen says: "Freeing space makes room. It does not make games faster,
unless a drive is almost full." (The blacklist entry "deleting temp files for
FPS" stays: this feature is for disk space, not speed.) When an SSD is over 90%
full, Doctor raises a "drive nearly full" finding pointing here.

## Drive health and maintenance

| Item | Source | Shown as |
|---|---|---|
| Health status | `MSFT_PhysicalDisk.HealthStatus` | Healthy, Warning, Unhealthy |
| Wear | `MSFT_StorageReliabilityCounter.Wear` | "18% of rated life used", or "not reported" |
| Temperature | Same class, `Temperature` and `TemperatureMax` | °C, with the drive's own limit |
| Errors | Read and write error totals | Any non-zero total is a warning |
| Last optimized | Optimize Drives history **(verify)** where it is readable | Date |
| Optimize now | `MSFT_Volume.Optimize` on a volume from the service's own volume list, never a client string | One-shot maintenance; nothing to revert |

## IPC (service side only)

| Method | Notes |
|---|---|
| `storage.health` | per physical disk: model, media type, health, wear, temperature, errors. No file names. |
| `storage.optimize` | `{ volume_id }` from `storage.health`'s list; refused while Session Mode is active |

Everything else is Tauri commands inside the app process.

## Amendment to `09-safety.md` (approved by the owner, 2026-11-23)

§3 "Deletes user files, caches, prefetch data, restore points or event logs"
becomes: "Verity never deletes anything on its own. On the Storage screen the
user may move files they pick to the Recycle Bin; that runs in the app under
the user's account, never in the service. Prefetch data, restore points and
event logs are never touched." The owner also approved permanent deletion for
items the Recycle Bin cannot take, after a second confirmation.

## Dependencies

None on M2 or M3: no system setting changes, and deletions go to the Recycle
Bin. Needs the existing app shell and `verity-sys` wrappers (new: shell file
operations, WMI/CIM reads).
