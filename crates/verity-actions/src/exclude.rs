//! Which processes background actions may touch (docs/05-actions.md,
//! "Exclusion list for background actions").

use std::collections::{HashMap, HashSet};

use verity_core::actions::{ProcInfo, ProcKey};

/// Anti-cheat processes; they and their games are never touched.
pub const ANTI_CHEAT: [&str; 14] = [
    "easyanticheat.exe",
    "easyanticheat_eos.exe",
    "beservice.exe",
    "beservice_x64.exe",
    "bedaisy.exe",
    "vgc.exe",
    "vgtray.exe",
    "faceit.exe",
    "faceitclient.exe",
    "faceitservice.exe",
    "eseaclient.exe",
    "gameguard.des",
    "ace-tray.exe",
    "sguard64.exe",
];

const LAUNCHERS: [&str; 20] = [
    "steam.exe",
    "steamwebhelper.exe",
    "steamservice.exe",
    "gameoverlayui.exe",
    "gameoverlayui64.exe",
    "epicgameslauncher.exe",
    "epicwebhelper.exe",
    "eosoverlayrenderer-win64-shipping.exe",
    "galaxyclient.exe",
    "galaxyclient helper.exe",
    "eadesktop.exe",
    "eabackgroundservice.exe",
    "ubisoftconnect.exe",
    "upc.exe",
    "battle.net.exe",
    "riotclientservices.exe",
    "riotclientux.exe",
    "xboxpcapp.exe",
    "gamingservices.exe",
    "rockstarservice.exe",
];

/// Voice chat, capture, streaming and overlays: slowing them breaks audio or recordings.
pub const VOICE_CAPTURE: [&str; 30] = [
    "discord.exe",
    "discordptb.exe",
    "discordcanary.exe",
    "obs64.exe",
    "obs32.exe",
    "streamlabs obs.exe",
    "streamlabs desktop.exe",
    "medal.exe",
    "medalencoder.exe",
    "nvcontainer.exe",
    "nvidia share.exe",
    "nvidia overlay.exe",
    "nvsphelper64.exe",
    "amdrsserv.exe",
    "radeonsoftware.exe",
    "gamebar.exe",
    "gamebarftserver.exe",
    "gamebarpresencewriter.exe",
    "ts3client_win64.exe",
    "teamspeak.exe",
    "mumble.exe",
    "zoom.exe",
    "ms-teams.exe",
    "teams.exe",
    "skype.exe",
    "voicemeeter.exe",
    "voicemeeterpro.exe",
    "voicemeeter8x64.exe",
    "voicemoddesktop.exe",
    "parsecd.exe",
];

const SECURITY: [&str; 18] = [
    "msmpeng.exe",
    "mssense.exe",
    "nissrv.exe",
    "securityhealthsystray.exe",
    "securityhealthservice.exe",
    "smartscreen.exe",
    "mbam.exe",
    "mbamservice.exe",
    "avp.exe",
    "avgui.exe",
    "avastui.exe",
    "avastsvc.exe",
    "ekrn.exe",
    "egui.exe",
    "bdagent.exe",
    "vsserv.exe",
    "nortonsecurity.exe",
    "mcshield.exe",
];

/// Parts of Windows that run in the user's session; slowing them makes the
/// desktop, input or sound stutter.
const SHELL: [&str; 18] = [
    "explorer.exe",
    "dwm.exe",
    "csrss.exe",
    "winlogon.exe",
    "sihost.exe",
    "ctfmon.exe",
    "fontdrvhost.exe",
    "textinputhost.exe",
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "searchhost.exe",
    "applicationframehost.exe",
    "lockapp.exe",
    "logonui.exe",
    "conhost.exe",
    "openconsole.exe",
    "audiodg.exe",
    "taskmgr.exe",
];

/// What the service knows about the session at the moment of the check.
pub struct Context<'a> {
    pub game: ProcKey,
    /// The Windows session the game runs in; other sessions are left alone.
    pub game_session: u32,
    pub own_pid: u32,
    /// Processes with an active audio session (reported by the agent).
    pub audio: &'a HashSet<u32>,
    /// Processes in front now or in the last 2 minutes (reported by the agent).
    pub recent_foreground: &'a HashSet<u32>,
    /// The profile's `extra_exclusions` (file names).
    pub extra: &'a [String],
}

fn listed(list: &[&str], name: &str) -> bool {
    list.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// The ancestors of `p` whose identity is consistent (each older than its
/// child, so a reused pid is not mistaken for a parent).
fn ancestors<'a>(p: &ProcInfo, by_pid: &HashMap<u32, &'a ProcInfo>) -> Vec<&'a ProcInfo> {
    let mut out = Vec::new();
    let (mut parent, mut born) = (p.parent, p.key.created);
    while let Some(a) = by_pid.get(&parent) {
        if a.key.created > born
            || out.len() > 64
            || a.key.pid == 0
            || out.iter().any(|o: &&ProcInfo| o.key == a.key)
        {
            break;
        }
        out.push(*a);
        (parent, born) = (a.parent, a.key.created);
    }
    out
}

/// Why `p` must not be touched, or None when a background action may apply.
pub fn excluded(
    p: &ProcInfo,
    by_pid: &HashMap<u32, &ProcInfo>,
    ctx: &Context,
) -> Option<&'static str> {
    let name = p.name.as_str();
    if p.key.created == 0 {
        return Some("Windows does not let Verity see it");
    }
    if p.session == 0 {
        return Some("system process");
    }
    if p.session != ctx.game_session {
        return Some("another user's session");
    }
    if p.key == ctx.game || ancestors(p, by_pid).iter().any(|a| a.key == ctx.game) {
        return Some("the game");
    }
    if let Some(g) = by_pid.get(&ctx.game.pid).filter(|g| g.key == ctx.game) {
        if ancestors(g, by_pid).iter().any(|a| a.key == p.key) {
            return Some("the game's launcher");
        }
    }
    if p.key.pid == ctx.own_pid || name.to_ascii_lowercase().starts_with("verity") {
        return Some("Verity");
    }
    if listed(&ANTI_CHEAT, name) {
        return Some("anti-cheat");
    }
    if listed(&LAUNCHERS, name) {
        return Some("game launcher");
    }
    if listed(&VOICE_CAPTURE, name) {
        return Some("voice chat, capture or overlay");
    }
    if listed(&SECURITY, name) {
        return Some("security software");
    }
    if listed(&SHELL, name) {
        return Some("part of Windows");
    }
    if ctx.audio.contains(&p.key.pid) {
        return Some("playing sound");
    }
    if ctx.recent_foreground.contains(&p.key.pid) {
        return Some("in front in the last 2 minutes");
    }
    if ctx.extra.iter().any(|e| e.eq_ignore_ascii_case(name)) {
        return Some("on your exclusion list");
    }
    None
}

/// The processes background actions may touch now.
pub fn eligible<'a>(procs: &'a [ProcInfo], ctx: &Context) -> Vec<&'a ProcInfo> {
    let by_pid: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.key.pid, p)).collect();
    procs
        .iter()
        .filter(|p| excluded(p, &by_pid, ctx).is_none())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, created: u64, parent: u32, session: u32, name: &str) -> ProcInfo {
        ProcInfo {
            key: ProcKey { pid, created },
            parent,
            session,
            name: name.into(),
        }
    }

    #[test]
    fn exclusions_follow_the_spec() {
        let procs = vec![
            proc(4, 1, 0, 0, "System"),
            proc(10, 5, 4, 1, "steam.exe"),
            proc(20, 9, 10, 1, "game.exe"),
            proc(21, 10, 20, 1, "crashpad_handler.exe"),
            proc(30, 6, 4, 1, "chrome.exe"),
            proc(31, 6, 4, 1, "spotify.exe"),
            proc(32, 6, 4, 1, "notepad.exe"),
            proc(33, 6, 4, 1, "Discord.exe"),
            proc(34, 6, 4, 1, "explorer.exe"),
            proc(35, 6, 4, 2, "other-user.exe"),
            proc(36, 0, 4, 1, "protected.exe"),
            proc(37, 6, 4, 1, "obs-mine.exe"),
            proc(38, 6, 4, 1, "EasyAntiCheat.exe"),
            proc(39, 6, 4, 1, "verity-ui.exe"),
            proc(40, 6, 4, 0, "svchost.exe"),
            // A pid reused after the game's parent link: older than the game,
            // so not its child.
            proc(50, 2, 20, 1, "old.exe"),
        ];
        let audio: HashSet<u32> = [31].into();
        let front: HashSet<u32> = [32].into();
        let extra = vec!["OBS-MINE.EXE".to_string()];
        let ctx = Context {
            game: ProcKey {
                pid: 20,
                created: 9,
            },
            game_session: 1,
            own_pid: 999,
            audio: &audio,
            recent_foreground: &front,
            extra: &extra,
        };
        let names: Vec<&str> = eligible(&procs, &ctx)
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, vec!["chrome.exe", "old.exe"]);

        let by_pid: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.key.pid, p)).collect();
        let why = |pid: u32| {
            excluded(
                procs.iter().find(|p| p.key.pid == pid).unwrap(),
                &by_pid,
                &ctx,
            )
        };
        assert_eq!(why(10), Some("the game's launcher"));
        assert_eq!(why(21), Some("the game"));
        assert_eq!(why(31), Some("playing sound"));
        assert_eq!(why(32), Some("in front in the last 2 minutes"));
        assert_eq!(why(40), Some("system process"));
        assert_eq!(why(35), Some("another user's session"));
        assert_eq!(why(38), Some("anti-cheat"));
    }
}
