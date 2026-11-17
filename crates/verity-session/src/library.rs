//! Library scan (docs/06-session-mode.md, "Library scan"): finds installed
//! games and the executable that actually renders, without starting anything.
//! The file system is behind [`LibraryFs`] so the logic runs against fakes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Steam,
    Epic,
    Gog,
    Xbox,
    Manual,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Steam => "steam",
            Source::Epic => "epic",
            Source::Gog => "gog",
            Source::Xbox => "xbox",
            Source::Manual => "manual",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "steam" => Source::Steam,
            "epic" => Source::Epic,
            "gog" => Source::Gog,
            "xbox" => Source::Xbox,
            "manual" => Source::Manual,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryGame {
    pub name: String,
    pub exe_path: PathBuf,
    pub source: Source,
    /// Steam app id, Epic app name or GOG id.
    pub launcher_id: Option<String>,
}

/// One GOG registry entry (`HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\<id>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GogEntry {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub exe: Option<PathBuf>,
}

/// What a scan reads. Every path here comes from the machine (registry,
/// launcher files), never from a client.
pub trait LibraryFs {
    fn read_to_string(&self, path: &Path) -> Option<String>;
    /// Files directly inside `dir` whose name ends with `ext` (lowercase, no dot).
    fn files(&self, dir: &Path, ext: &str) -> Vec<PathBuf>;
    /// Executables under `dir`, at most `depth` folders down: (path, size in bytes).
    fn exes(&self, dir: &Path, depth: usize) -> Vec<(PathBuf, u64)>;
    fn subdirs(&self, dir: &Path) -> Vec<PathBuf>;
    fn is_file(&self, path: &Path) -> bool;
}

/// Where the launchers live on this machine.
#[derive(Debug, Clone, Default)]
pub struct Roots {
    pub steam: Option<PathBuf>,
    pub epic_manifests: Option<PathBuf>,
    pub gog: Vec<GogEntry>,
    /// Root of each fixed volume, e.g. `C:\`.
    pub volumes: Vec<PathBuf>,
}

/// How deep the main-exe search goes; Unreal games keep theirs at
/// `<game>\Binaries\Win64\<game>-Win64-Shipping.exe`.
const EXE_DEPTH: usize = 5;

/// Steam app ids that install as apps but are not games.
const STEAM_TOOLS: [&str; 6] = [
    "228980",  // Steamworks Common Redistributables
    "1070560", // Steam Linux Runtime
    "1391110", // Steam Linux Runtime - Soldier
    "1628350", // Steam Linux Runtime - Sniper
    "250820",  // SteamVR
    "1493710", // Proton Experimental
];

/// `StateFlags` bit Steam sets once an app is fully installed.
const STEAM_FULLY_INSTALLED: u64 = 4;

pub fn scan(fs: &dyn LibraryFs, roots: &Roots) -> Vec<LibraryGame> {
    let mut out = Vec::new();
    if let Some(steam) = &roots.steam {
        out.extend(scan_steam(fs, steam));
    }
    if let Some(dir) = &roots.epic_manifests {
        out.extend(scan_epic(fs, dir));
    }
    out.extend(scan_gog(fs, &roots.gog));
    for v in &roots.volumes {
        out.extend(scan_xbox(fs, &v.join("XboxGames")));
    }
    // The same exe found twice (say a Steam copy registered by hand) keeps the first.
    let mut seen = std::collections::HashSet::new();
    out.retain(|g| seen.insert(g.exe_path.to_string_lossy().to_lowercase()));
    out
}

fn scan_steam(fs: &dyn LibraryFs, steam: &Path) -> Vec<LibraryGame> {
    let mut libraries = vec![steam.to_path_buf()];
    if let Some(text) = fs.read_to_string(&steam.join(r"steamapps\libraryfolders.vdf")) {
        if let Some(Kv::Obj(folders)) = parse_vdf(&text).get("libraryfolders") {
            for (_, folder) in folders {
                if let Some(path) = folder.get("path").and_then(Kv::as_str) {
                    let p = PathBuf::from(path);
                    if !libraries.iter().any(|l| same_path(l, &p)) {
                        libraries.push(p);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for lib in libraries {
        let apps = lib.join("steamapps");
        for acf in fs.files(&apps, "acf") {
            let Some(text) = fs.read_to_string(&acf) else {
                continue;
            };
            let kv = parse_vdf(&text);
            let Some(state) = kv.get("AppState") else {
                continue;
            };
            let field = |k: &str| state.get(k).and_then(Kv::as_str).map(str::to_string);
            let (Some(appid), Some(name), Some(dir)) =
                (field("appid"), field("name"), field("installdir"))
            else {
                continue;
            };
            let flags: u64 = field("StateFlags")
                .and_then(|f| f.parse().ok())
                .unwrap_or(0);
            if STEAM_TOOLS.contains(&appid.as_str()) || flags & STEAM_FULLY_INSTALLED == 0 {
                continue;
            }
            let root = apps.join("common").join(&dir);
            if let Some(exe) = pick_main_exe(&root, &fs.exes(&root, EXE_DEPTH)) {
                out.push(LibraryGame {
                    name,
                    exe_path: exe,
                    source: Source::Steam,
                    launcher_id: Some(appid),
                });
            }
        }
    }
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EpicItem {
    display_name: String,
    install_location: String,
    #[serde(default)]
    launch_executable: String,
    #[serde(default)]
    app_name: Option<String>,
    #[serde(default, rename = "bIsIncompleteInstall")]
    incomplete: bool,
    #[serde(default)]
    app_categories: Vec<String>,
}

fn scan_epic(fs: &dyn LibraryFs, dir: &Path) -> Vec<LibraryGame> {
    let mut out = Vec::new();
    for item in fs.files(dir, "item") {
        let Some(text) = fs.read_to_string(&item) else {
            continue;
        };
        let Ok(m) = serde_json::from_str::<EpicItem>(&text) else {
            continue;
        };
        // Engine and plugin installs carry other categories; no list means a game.
        let is_game = m.app_categories.is_empty()
            || m.app_categories
                .iter()
                .any(|c| c.eq_ignore_ascii_case("games"));
        if m.incomplete || !is_game || m.install_location.is_empty() {
            continue;
        }
        let root = PathBuf::from(&m.install_location);
        let exes = fs.exes(&root, EXE_DEPTH);
        let launch = (!m.launch_executable.is_empty()).then(|| root.join(&m.launch_executable));
        if let Some(exe) = prefer_shipping(&root, &exes, launch.filter(|p| fs.is_file(p))) {
            out.push(LibraryGame {
                name: m.display_name,
                exe_path: exe,
                source: Source::Epic,
                launcher_id: m.app_name,
            });
        }
    }
    out
}

fn scan_gog(fs: &dyn LibraryFs, entries: &[GogEntry]) -> Vec<LibraryGame> {
    entries
        .iter()
        .filter_map(|g| {
            let exes = fs.exes(&g.path, EXE_DEPTH);
            let listed = g.exe.clone().filter(|p| fs.is_file(p));
            prefer_shipping(&g.path, &exes, listed).map(|exe| LibraryGame {
                name: g.name.clone(),
                exe_path: exe,
                source: Source::Gog,
                launcher_id: Some(g.id.clone()),
            })
        })
        .collect()
}

fn scan_xbox(fs: &dyn LibraryFs, root: &Path) -> Vec<LibraryGame> {
    fs.subdirs(root)
        .into_iter()
        .filter_map(|game| {
            let content = game.join("Content");
            let exe = pick_main_exe(&content, &fs.exes(&content, EXE_DEPTH))?;
            let name = game.file_name()?.to_string_lossy().into_owned();
            Some(LibraryGame {
                name,
                exe_path: exe,
                source: Source::Xbox,
                launcher_id: None,
            })
        })
        .collect()
}

/// A launcher-listed exe is often a small bootstrapper that starts the real
/// Unreal `*-Shipping.exe`; the process that renders is the one to match.
fn prefer_shipping(
    root: &Path,
    exes: &[(PathBuf, u64)],
    listed: Option<PathBuf>,
) -> Option<PathBuf> {
    let shipping = exes
        .iter()
        .filter(|(p, _)| file_lower(p).ends_with("-shipping.exe") && eligible(root, p))
        .max_by_key(|(_, size)| *size)
        .map(|(p, _)| p.clone());
    shipping.or(listed).or_else(|| pick_main_exe(root, exes))
}

/// Names that are never the game itself: crash handlers, installers, helpers.
const NOT_MAIN: [&str; 17] = [
    "crash",
    "redist",
    "setup",
    "install",
    "unins",
    "webview",
    "helper",
    "launcher",
    "updater",
    "report",
    "prereq",
    "easyanticheat",
    "battleye",
    "cefprocess",
    "subprocess",
    "dotnet",
    "protocolselector",
];

/// Folders that only hold redistributables and support tools.
const NOT_MAIN_DIRS: [&str; 10] = [
    "_commonredist",
    "redist",
    "redistributables",
    "__installer",
    "directx",
    "easyanticheat",
    "battleye",
    "webview2",
    "prerequisites",
    "installers",
];

fn file_lower(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn eligible(root: &Path, exe: &Path) -> bool {
    let name = file_lower(exe);
    if NOT_MAIN.iter().any(|n| name.contains(n)) {
        return false;
    }
    let rel = exe.strip_prefix(root).unwrap_or(exe);
    !rel.parent()
        .into_iter()
        .flat_map(Path::components)
        .any(|c| {
            let c = c.as_os_str().to_string_lossy().to_lowercase();
            NOT_MAIN_DIRS.contains(&c.as_str())
        })
}

/// The largest exe that is not an installer, crash handler or redistributable.
/// shortcut: Steam's launch config lives in the binary `appinfo.vdf`; it is not
/// read, so a game whose biggest exe is not the renderer needs a manual add.
pub fn pick_main_exe(root: &Path, exes: &[(PathBuf, u64)]) -> Option<PathBuf> {
    exes.iter()
        .filter(|(p, _)| eligible(root, p))
        .max_by_key(|(_, size)| *size)
        .map(|(p, _)| p.clone())
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// Processes that are never games, whatever they draw: launchers, helpers,
/// anti-cheat services, browsers, players, capture and chat apps.
pub const NEVER_A_GAME: [&str; 44] = [
    "steam.exe",
    "steamwebhelper.exe",
    "steamservice.exe",
    "gameoverlayui.exe",
    "epicgameslauncher.exe",
    "epicwebhelper.exe",
    "unrealcefsubprocess.exe",
    "galaxyclient.exe",
    "galaxyclient helper.exe",
    "gogalaxy.exe",
    "eadesktop.exe",
    "ubisoftconnect.exe",
    "battle.net.exe",
    "xboxpcapp.exe",
    "gamebar.exe",
    "crashreportclient.exe",
    "unitycrashhandler64.exe",
    "unitycrashhandler32.exe",
    "crashpad_handler.exe",
    "easyanticheat.exe",
    "easyanticheat_eos.exe",
    "beservice.exe",
    "vgc.exe",
    "explorer.exe",
    "dwm.exe",
    "msedgewebview2.exe",
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "opera.exe",
    "brave.exe",
    "vivaldi.exe",
    "vlc.exe",
    "mpc-hc64.exe",
    "potplayermini64.exe",
    "wmplayer.exe",
    "video.ui.exe",
    "obs64.exe",
    "discord.exe",
    "spotify.exe",
    "teams.exe",
    "zoom.exe",
    "verity-ui.exe",
    "nvcontainer.exe",
];

pub fn never_a_game(exe_name: &str) -> bool {
    NEVER_A_GAME
        .iter()
        .any(|n| n.eq_ignore_ascii_case(exe_name))
}

/// A Valve KeyValues (text VDF) node.
#[derive(Debug, Clone, PartialEq)]
pub enum Kv {
    Str(String),
    Obj(Vec<(String, Kv)>),
}

impl Kv {
    /// Keys are case-insensitive in KeyValues.
    pub fn get(&self, key: &str) -> Option<&Kv> {
        match self {
            Kv::Obj(items) => items
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Kv::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Kv::Str(s) => Some(s),
            Kv::Obj(_) => None,
        }
    }
}

/// Parses text VDF (`libraryfolders.vdf`, `appmanifest_*.acf`). Malformed input
/// yields whatever parsed before the error; it never panics.
pub fn parse_vdf(text: &str) -> Kv {
    let tokens = tokenize(text);
    let mut pos = 0;
    Kv::Obj(parse_obj(&tokens, &mut pos, 0))
}

#[derive(Debug, PartialEq)]
enum Tok {
    Str(String),
    Open,
    Close,
}

fn tokenize(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => out.push(Tok::Open),
            '}' => out.push(Tok::Close),
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => break,
                        },
                        _ => s.push(c),
                    }
                }
                out.push(Tok::Str(s));
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {}
            _ => {
                // Unquoted token, up to whitespace or a brace.
                let mut s = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || n == '{' || n == '}' || n == '"' {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                out.push(Tok::Str(s));
            }
        }
    }
    out
}

/// Nesting deeper than this is treated as malformed (bounds recursion).
const MAX_DEPTH: usize = 32;

fn parse_obj(tokens: &[Tok], pos: &mut usize, depth: usize) -> Vec<(String, Kv)> {
    let mut items = Vec::new();
    while let Some(tok) = tokens.get(*pos) {
        match tok {
            Tok::Close => {
                *pos += 1;
                break;
            }
            Tok::Open => {
                // A value without a key: skip it.
                *pos += 1;
                if depth < MAX_DEPTH {
                    parse_obj(tokens, pos, depth + 1);
                }
            }
            Tok::Str(key) => {
                *pos += 1;
                match tokens.get(*pos) {
                    Some(Tok::Str(v)) => {
                        *pos += 1;
                        items.push((key.clone(), Kv::Str(v.clone())));
                    }
                    Some(Tok::Open) if depth < MAX_DEPTH => {
                        *pos += 1;
                        let obj = parse_obj(tokens, pos, depth + 1);
                        items.push((key.clone(), Kv::Obj(obj)));
                    }
                    _ => break,
                }
            }
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeFs {
        files: HashMap<String, String>,
        exes: Vec<(String, u64)>,
    }

    fn key(p: &Path) -> String {
        p.to_string_lossy().to_lowercase()
    }

    impl FakeFs {
        fn file(mut self, p: &str, text: &str) -> Self {
            self.files.insert(p.to_lowercase(), text.into());
            self
        }
        fn exe(mut self, p: &str, size: u64) -> Self {
            self.exes.push((p.into(), size));
            self
        }
    }

    impl LibraryFs for FakeFs {
        fn read_to_string(&self, path: &Path) -> Option<String> {
            self.files.get(&key(path)).cloned()
        }
        fn files(&self, dir: &Path, ext: &str) -> Vec<PathBuf> {
            let d = key(dir) + "\\";
            self.files
                .keys()
                .filter(|k| k.starts_with(&d) && !k[d.len()..].contains('\\') && k.ends_with(ext))
                .map(PathBuf::from)
                .collect()
        }
        fn exes(&self, dir: &Path, _depth: usize) -> Vec<(PathBuf, u64)> {
            let d = key(dir) + "\\";
            self.exes
                .iter()
                .filter(|(p, _)| p.to_lowercase().starts_with(&d))
                .map(|(p, s)| (PathBuf::from(p), *s))
                .collect()
        }
        fn subdirs(&self, dir: &Path) -> Vec<PathBuf> {
            let d = key(dir) + "\\";
            let mut out: Vec<PathBuf> = self
                .exes
                .iter()
                .filter_map(|(p, _)| {
                    let rest = p.to_lowercase().strip_prefix(&d)?.to_string();
                    Some(dir.join(&p[d.len()..d.len() + rest.find('\\')?]))
                })
                .collect();
            out.dedup();
            out
        }
        fn is_file(&self, path: &Path) -> bool {
            self.files.contains_key(&key(path))
                || self
                    .exes
                    .iter()
                    .any(|(p, _)| p.eq_ignore_ascii_case(&key(path)))
        }
    }

    // Copied from the dev laptop's Steam install (2026-11-17).
    const LIBRARYFOLDERS: &str = r#""libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
		"apps"
		{
			"228980"		"157818239"
			"1551360"		"159159767186"
		}
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}"#;

    fn acf(appid: &str, name: &str, dir: &str, flags: u32) -> String {
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\t\
             \"StateFlags\"\t\t\"{flags}\"\n\t\"installdir\"\t\t\"{dir}\"\n}}\n"
        )
    }

    const STEAM: &str = r"C:\Program Files (x86)\Steam";

    fn steam_fs() -> FakeFs {
        let apps = format!(r"{STEAM}\steamapps");
        let common = format!(r"{apps}\common");
        FakeFs::default()
            .file(&format!(r"{apps}\libraryfolders.vdf"), LIBRARYFOLDERS)
            .file(
                &format!(r"{apps}\appmanifest_1551360.acf"),
                &acf("1551360", "Forza Horizon 5", "ForzaHorizon5", 4),
            )
            .file(
                &format!(r"{apps}\appmanifest_228980.acf"),
                &acf(
                    "228980",
                    "Steamworks Common Redistributables",
                    "Steamworks Shared",
                    4,
                ),
            )
            .file(
                &format!(r"{apps}\appmanifest_349620.acf"),
                &acf("349620", "BlastZone 2 Demo", "BlastZone 2 Demo", 1538),
            )
            .file(
                &format!(r"{apps}\appmanifest_3949040.acf"),
                &acf("3949040", "RV There Yet?", "Ride", 518),
            )
            .file(
                r"D:\SteamLibrary\steamapps\appmanifest_4747510.acf",
                &acf("4747510", "BOMBANANA! Demo", "BOMBANANA! Demo", 4),
            )
            .exe(
                &format!(r"{common}\ForzaHorizon5\ForzaHorizon5.exe"),
                176_231_056,
            )
            .exe(
                &format!(r"{common}\ForzaHorizon5\webview2\msedgewebview2.exe"),
                2_817_920,
            )
            .exe(
                &format!(r"{common}\ForzaHorizon5\ForzaProtocolSelector.exe"),
                27_136,
            )
            .exe(
                &format!(
                    r"{common}\Steamworks Shared\_CommonRedist\vcredist\2022\VC_redist.x64.exe"
                ),
                18_731_856,
            )
            .exe(
                &format!(r"{common}\Ride\Ride\Binaries\Win64\Ride-Win64-Shipping.exe"),
                181_991_424,
            )
            .exe(
                &format!(r"{common}\Ride\Engine\Binaries\Win64\CrashReportClient.exe"),
                27_913_728,
            )
            .exe(
                &format!(r"{common}\Ride\Engine\Extras\Redist\en-us\vc_redist.x64.exe"),
                25_641_968,
            )
            .exe(&format!(r"{common}\Ride\Ride.exe"), 169_472)
            .exe(
                r"D:\SteamLibrary\steamapps\common\BOMBANANA! Demo\UnityCrashHandler64.exe",
                1_609_640,
            )
            .exe(
                r"D:\SteamLibrary\steamapps\common\BOMBANANA! Demo\BOMBANANA.exe",
                667_648,
            )
    }

    #[test]
    fn steam_finds_games_and_their_rendering_exe() {
        let roots = Roots {
            steam: Some(STEAM.into()),
            ..Default::default()
        };
        let mut games = scan(&steam_fs(), &roots);
        games.sort_by(|a, b| a.name.cmp(&b.name));
        let got: Vec<(&str, String)> = games
            .iter()
            .map(|g| (g.name.as_str(), file_lower(&g.exe_path)))
            .collect();
        assert_eq!(
            got,
            [
                ("BOMBANANA! Demo", "bombanana.exe".to_string()),
                ("Forza Horizon 5", "forzahorizon5.exe".to_string()),
                ("RV There Yet?", "ride-win64-shipping.exe".to_string()),
            ]
        );
        assert!(games.iter().all(|g| g.source == Source::Steam));
        assert_eq!(games[1].launcher_id.as_deref(), Some("1551360"));
    }

    #[test]
    fn steam_missing_or_broken_files_find_nothing() {
        let roots = Roots {
            steam: Some(STEAM.into()),
            ..Default::default()
        };
        assert!(scan(&FakeFs::default(), &roots).is_empty());
        let broken = FakeFs::default()
            .file(
                &format!(r"{STEAM}\steamapps\libraryfolders.vdf"),
                "\"x\" {{{",
            )
            .file(
                &format!(r"{STEAM}\steamapps\appmanifest_1.acf"),
                "\"AppState\" { \"appid\"",
            );
        assert!(scan(&broken, &roots).is_empty());
    }

    #[test]
    fn epic_reads_manifests_and_prefers_the_shipping_exe() {
        let dir = r"C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests";
        let item = |name: &str, loc: &str, exe: &str, extra: &str| {
            format!(
                r#"{{"DisplayName":"{name}","InstallLocation":"{loc}","LaunchExecutable":"{exe}","AppName":"{name}id"{extra}}}"#
            )
        };
        let fs = FakeFs::default()
            .file(
                &format!(r"{dir}\A.item"),
                &item(
                    "Plain",
                    r"D:\\Epic\\Plain",
                    "Plain.exe",
                    r#","AppCategories":["public","games"]"#,
                ),
            )
            .file(
                &format!(r"{dir}\B.item"),
                &item("Unreal", r"D:\\Epic\\Unreal", "Unreal.exe", ""),
            )
            .file(
                &format!(r"{dir}\C.item"),
                &item(
                    "Half",
                    r"D:\\Epic\\Half",
                    "Half.exe",
                    r#","bIsIncompleteInstall":true"#,
                ),
            )
            .file(
                &format!(r"{dir}\D.item"),
                &item(
                    "Engine",
                    r"D:\\Epic\\UE_5",
                    "x.exe",
                    r#","AppCategories":["engines"]"#,
                ),
            )
            .file(&format!(r"{dir}\E.item"), "not json")
            .exe(r"D:\Epic\Plain\Plain.exe", 50)
            .exe(r"D:\Epic\Plain\Bigger.exe", 900)
            .exe(r"D:\Epic\Unreal\Unreal.exe", 100)
            .exe(
                r"D:\Epic\Unreal\Unreal\Binaries\Win64\Unreal-Win64-Shipping.exe",
                9_000,
            )
            .exe(r"D:\Epic\Half\Half.exe", 10)
            .exe(r"D:\Epic\UE_5\x.exe", 10);
        let fs = fs
            .file(r"D:\Epic\Plain\Plain.exe", "")
            .file(r"D:\Epic\Unreal\Unreal.exe", "");
        let roots = Roots {
            epic_manifests: Some(dir.into()),
            ..Default::default()
        };
        let mut games = scan(&fs, &roots);
        games.sort_by(|a, b| a.name.cmp(&b.name));
        let got: Vec<(&str, String)> = games
            .iter()
            .map(|g| (g.name.as_str(), file_lower(&g.exe_path)))
            .collect();
        // The launch executable wins over a bigger exe, a Shipping exe over both.
        assert_eq!(
            got,
            [
                ("Plain", "plain.exe".to_string()),
                ("Unreal", "unreal-win64-shipping.exe".to_string()),
            ]
        );
    }

    #[test]
    fn gog_and_xbox() {
        let fs = FakeFs::default()
            .exe(r"D:\GOG\Witcher\bin\x64\witcher3.exe", 80)
            .exe(r"D:\GOG\Witcher\unins000.exe", 900)
            .exe(r"D:\GOG\Gone\gone.exe", 1)
            .exe(r"E:\XboxGames\Halo\Content\halo.exe", 500)
            .exe(r"E:\XboxGames\Halo\Content\Installers\setup.exe", 900)
            .file(r"D:\GOG\Witcher\bin\x64\witcher3.exe", "");
        let roots = Roots {
            gog: vec![
                GogEntry {
                    id: "1".into(),
                    name: "The Witcher 3".into(),
                    path: r"D:\GOG\Witcher".into(),
                    exe: Some(r"D:\GOG\Witcher\bin\x64\witcher3.exe".into()),
                },
                GogEntry {
                    id: "2".into(),
                    name: "Gone".into(),
                    path: r"D:\GOG\Gone".into(),
                    // Listed exe missing: falls back to the largest eligible one.
                    exe: Some(r"D:\GOG\Gone\missing.exe".into()),
                },
            ],
            volumes: vec![r"E:\".into()],
            ..Default::default()
        };
        let games = scan(&fs, &roots);
        let got: Vec<(&str, String, Source)> = games
            .iter()
            .map(|g| (g.name.as_str(), file_lower(&g.exe_path), g.source))
            .collect();
        assert_eq!(
            got,
            [
                ("The Witcher 3", "witcher3.exe".to_string(), Source::Gog),
                ("Gone", "gone.exe".to_string(), Source::Gog),
                ("Halo", "halo.exe".to_string(), Source::Xbox),
            ]
        );
    }

    #[test]
    fn main_exe_rules() {
        let root = Path::new(r"C:\G");
        let e = |p: &str, s| (PathBuf::from(p), s);
        assert_eq!(pick_main_exe(root, &[]), None);
        // Only helpers: no game exe.
        assert_eq!(
            pick_main_exe(root, &[e(r"C:\G\UnityCrashHandler64.exe", 9)]),
            None
        );
        assert_eq!(
            pick_main_exe(
                root,
                &[
                    e(r"C:\G\game.exe", 10),
                    e(r"C:\G\GameLauncher.exe", 99),
                    e(r"C:\G\redist\big.exe", 999),
                ]
            ),
            Some(r"C:\G\game.exe".into())
        );
    }

    #[test]
    fn vdf_parser() {
        let kv = parse_vdf(
            "// comment\n\"A\" { \"k\" \"v \\\"q\\\"\" \"Sub\" { \"x\" \"1\" } bare value }",
        );
        let a = kv.get("a").unwrap();
        assert_eq!(a.get("K").and_then(Kv::as_str), Some("v \"q\""));
        assert_eq!(
            a.get("sub").and_then(|s| s.get("x")).and_then(Kv::as_str),
            Some("1")
        );
        assert_eq!(a.get("bare").and_then(Kv::as_str), Some("value"));
        // Deep nesting does not overflow the stack.
        let deep = "\"a\" {".repeat(10_000);
        let _ = parse_vdf(&deep);
        assert_eq!(parse_vdf(""), Kv::Obj(vec![]));
    }

    #[test]
    fn never_a_game_is_case_insensitive() {
        assert!(never_a_game("Steam.exe"));
        assert!(never_a_game("CHROME.EXE"));
        assert!(!never_a_game("ForzaHorizon5.exe"));
    }
}
