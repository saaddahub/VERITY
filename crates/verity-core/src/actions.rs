//! What reversible actions need from Windows (docs/05-actions.md), and the
//! journal they are recorded in. `verity-sys` implements [`ActionSys`];
//! `verity-store` implements [`JournalStore`]; tests use fakes.

use serde::{Deserialize, Serialize};

use crate::OsError;

/// A process identity: its pid plus its creation time (FILETIME). A bare pid
/// can be reused by another process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcKey {
    pub pid: u32,
    pub created: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub key: ProcKey,
    pub parent: u32,
    /// Windows session; 0 holds services and system processes.
    pub session: u32,
    /// File name only, like `chrome.exe`.
    pub name: String,
}

/// EcoQoS as Windows reports it: the bits the process asked to control, and
/// their values (`PROCESS_POWER_THROTTLING_STATE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Throttle {
    pub control: u32,
    pub state: u32,
}

/// `PROCESS_POWER_THROTTLING_EXECUTION_SPEED`.
pub const EXECUTION_SPEED: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProcError {
    #[error("the process has exited")]
    Gone,
    /// Protected processes and other users' processes; a normal result.
    #[error("Windows refused access to the process")]
    Denied,
    #[error(transparent)]
    Os(#[from] OsError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum RegValue {
    Dword(u32),
    Str(String),
    Binary(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub bits: u32,
    pub hz: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupPlace {
    /// `HKCU\...\CurrentVersion\Run`.
    Run,
    /// The user's Startup folder.
    StartupFolder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupItem {
    pub name: String,
    pub place: StartupPlace,
    pub enabled: bool,
}

/// The Windows calls actions make. The service uses the process and power
/// parts; the user agent uses the registry (current user only) and display parts.
pub trait ActionSys: Send + Sync {
    fn processes(&self) -> Result<Vec<ProcInfo>, OsError>;
    fn throttling(&self, p: ProcKey) -> Result<Throttle, ProcError>;
    fn set_throttling(&self, p: ProcKey, t: Throttle) -> Result<(), ProcError>;
    fn default_cpu_sets(&self, p: ProcKey) -> Result<Vec<u32>, ProcError>;
    /// An empty list clears the process's default CPU sets.
    fn set_default_cpu_sets(&self, p: ProcKey, sets: &[u32]) -> Result<(), ProcError>;
    /// CPU set ids of the efficiency cores: those with the lowest efficiency
    /// class. None when every core has the same class (not a hybrid CPU).
    fn efficiency_cpu_sets(&self) -> Result<Option<Vec<u32>>, OsError>;

    /// None when Windows does not know the power source.
    fn on_ac(&self) -> Result<Option<bool>, OsError>;
    /// The power mode overlay GUID in force while plugged in, lowercase. None
    /// when this Windows has no power-mode API.
    fn power_overlay(&self) -> Result<Option<String>, OsError>;
    fn set_power_overlay(&self, guid: &str) -> Result<(), OsError>;

    /// Values under `HKEY_CURRENT_USER\<key>`.
    fn reg_get(&self, key: &str, name: &str) -> Result<Option<RegValue>, OsError>;
    fn reg_set(&self, key: &str, name: &str, v: &RegValue) -> Result<(), OsError>;
    /// Deleting a value that does not exist succeeds.
    fn reg_delete(&self, key: &str, name: &str) -> Result<(), OsError>;
    /// The current user's startup programs and whether each is enabled.
    fn startup_items(&self) -> Result<Vec<StartupItem>, OsError>;

    /// Device name of the primary display, like `\\.\DISPLAY1`.
    fn primary_display(&self) -> Result<String, OsError>;
    fn display_mode(&self, device: &str) -> Result<DisplayMode, OsError>;
    fn display_modes(&self, device: &str) -> Result<Vec<DisplayMode>, OsError>;
    /// Tests the mode first, then applies it and saves it for the user.
    fn set_display_hz(&self, device: &str, hz: u32) -> Result<(), OsError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Session,
    Persistent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Executor {
    Service,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalState {
    Pending,
    Applied,
    Reverted,
    RevertFailed,
    Superseded,
}

impl JournalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Reverted => "reverted",
            Self::RevertFailed => "revert_failed",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "applied" => Self::Applied,
            "reverted" => Self::Reverted,
            "revert_failed" => Self::RevertFailed,
            "superseded" => Self::Superseded,
            _ => return None,
        })
    }

    /// Still in effect, or possibly in effect (a crash between the journal
    /// write and the change).
    pub fn open(self) -> bool {
        matches!(self, Self::Pending | Self::Applied)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewJournalRow {
    /// Groups the rows of one session or one user request.
    pub txn_id: String,
    pub action_id: String,
    pub scope: Scope,
    pub executor: Executor,
    pub params: serde_json::Value,
    pub snapshot: serde_json::Value,
    /// The user an agent row belongs to (their SID); empty for service rows.
    pub owner: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalRow {
    pub id: i64,
    #[serde(flatten)]
    pub row: NewJournalRow,
    pub state: JournalState,
    pub created_utc: String,
    pub applied_utc: Option<String>,
    pub reverted_utc: Option<String>,
    pub error: Option<String>,
}

/// Which rows to list.
#[derive(Debug, Clone, Default)]
pub struct JournalFilter<'a> {
    pub txn_id: Option<&'a str>,
    pub scope: Option<Scope>,
    pub executor: Option<Executor>,
    pub owner: Option<&'a str>,
    pub open_only: bool,
}

/// Durable storage for journal rows. Every write is committed before it returns.
pub trait JournalStore {
    fn journal_insert(&self, row: &NewJournalRow) -> Result<i64, String>;
    fn journal_set_state(
        &self,
        id: i64,
        state: JournalState,
        error: Option<&str>,
    ) -> Result<(), String>;
    fn journal_get(&self, id: i64) -> Result<Option<JournalRow>, String>;
    /// Rows in the order they were written.
    fn journal_list(&self, filter: &JournalFilter) -> Result<Vec<JournalRow>, String>;
}
