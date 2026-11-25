//! In-memory Windows and journal for unit tests.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use verity_core::actions::{
    ActionSys, DisplayMode, JournalFilter, JournalRow, JournalState, JournalStore, NewJournalRow,
    ProcError, ProcInfo, ProcKey, RegValue, StartupItem, Throttle,
};
use verity_core::OsError;

#[derive(Default)]
struct State {
    procs: BTreeMap<u32, (ProcInfo, Throttle, Vec<u32>)>,
    listings: usize,
    denied: Vec<u32>,
    fail_sets: bool,
    efficiency: Option<Vec<u32>>,
    ac: Option<bool>,
    overlay: Option<String>,
    reg: HashMap<(String, String), RegValue>,
    startup: Vec<StartupItem>,
    modes: Vec<DisplayMode>,
    current: Option<DisplayMode>,
}

#[derive(Default)]
pub struct FakeSys(Mutex<State>);

fn os(api: &'static str) -> OsError {
    OsError { api, code: 5 }
}

impl FakeSys {
    fn s(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap()
    }
    pub fn add_proc(&self, key: ProcKey, session: u32, name: &str) {
        self.add_child(key, session, name, 4);
    }
    pub fn add_child(&self, key: ProcKey, session: u32, name: &str, parent: u32) {
        let info = ProcInfo {
            key,
            parent,
            session,
            name: name.into(),
        };
        self.s()
            .procs
            .insert(key.pid, (info, Throttle::default(), Vec::new()));
    }
    /// How many times the process list was read (it is slow on real Windows).
    pub fn listings(&self) -> usize {
        self.s().listings
    }
    pub fn remove_proc(&self, pid: u32) {
        self.s().procs.remove(&pid);
    }
    pub fn deny(&self, pid: u32) {
        self.s().denied.push(pid);
    }
    pub fn fail_sets(&self, on: bool) {
        self.s().fail_sets = on;
    }
    pub fn throttle(&self, k: ProcKey) -> Throttle {
        self.s().procs[&k.pid].1
    }
    pub fn set_throttle(&self, k: ProcKey, t: Throttle) {
        self.s().procs.get_mut(&k.pid).unwrap().1 = t;
    }
    pub fn sets(&self, k: ProcKey) -> Vec<u32> {
        self.s().procs[&k.pid].2.clone()
    }
    pub fn set_efficiency(&self, e: Option<Vec<u32>>) {
        self.s().efficiency = e;
    }
    pub fn set_power(&self, ac: Option<bool>, overlay: Option<&str>) {
        let mut s = self.s();
        s.ac = ac;
        s.overlay = overlay.map(str::to_string);
    }
    pub fn overlay(&self) -> Option<String> {
        self.s().overlay.clone()
    }
    pub fn reg(&self, key: &str, name: &str) -> Option<RegValue> {
        self.s().reg.get(&(key.into(), name.into())).cloned()
    }
    pub fn put_reg(&self, key: &str, name: &str, v: RegValue) {
        self.s().reg.insert((key.into(), name.into()), v);
    }
    pub fn set_startup(&self, items: Vec<StartupItem>) {
        self.s().startup = items;
    }
    pub fn set_display(&self, current: DisplayMode, modes: Vec<DisplayMode>) {
        let mut s = self.s();
        s.current = Some(current);
        s.modes = modes;
    }
    pub fn hz(&self) -> u32 {
        self.s().current.unwrap().hz
    }

    fn live(&self, k: ProcKey) -> Result<(), ProcError> {
        let s = self.s();
        match s.procs.get(&k.pid) {
            Some((i, _, _)) if i.key == k => {}
            _ => return Err(ProcError::Gone),
        }
        if s.denied.contains(&k.pid) {
            return Err(ProcError::Denied);
        }
        Ok(())
    }
}

impl ActionSys for FakeSys {
    fn processes(&self) -> Result<Vec<ProcInfo>, OsError> {
        let mut s = self.s();
        s.listings += 1;
        Ok(s.procs.values().map(|p| p.0.clone()).collect())
    }
    fn throttling(&self, p: ProcKey) -> Result<Throttle, ProcError> {
        self.live(p)?;
        Ok(self.throttle(p))
    }
    fn set_throttling(&self, p: ProcKey, t: Throttle) -> Result<(), ProcError> {
        self.live(p)?;
        if self.s().fail_sets && t.state != 0 {
            return Err(ProcError::Os(os("SetProcessInformation")));
        }
        self.set_throttle(p, t);
        Ok(())
    }
    fn default_cpu_sets(&self, p: ProcKey) -> Result<Vec<u32>, ProcError> {
        self.live(p)?;
        Ok(self.sets(p))
    }
    fn set_default_cpu_sets(&self, p: ProcKey, sets: &[u32]) -> Result<(), ProcError> {
        self.live(p)?;
        self.s().procs.get_mut(&p.pid).unwrap().2 = sets.to_vec();
        Ok(())
    }
    fn efficiency_cpu_sets(&self) -> Result<Option<Vec<u32>>, OsError> {
        Ok(self.s().efficiency.clone())
    }
    fn on_ac(&self) -> Result<Option<bool>, OsError> {
        Ok(self.s().ac)
    }
    fn power_overlay(&self) -> Result<Option<String>, OsError> {
        Ok(self.s().overlay.clone())
    }
    fn set_power_overlay(&self, guid: &str) -> Result<(), OsError> {
        let mut s = self.s();
        if s.overlay.is_none() {
            return Err(os("PowerSetActiveOverlayScheme"));
        }
        s.overlay = Some(guid.into());
        Ok(())
    }
    fn reg_get(&self, key: &str, name: &str) -> Result<Option<RegValue>, OsError> {
        Ok(self.reg(key, name))
    }
    fn reg_set(&self, key: &str, name: &str, v: &RegValue) -> Result<(), OsError> {
        self.put_reg(key, name, v.clone());
        Ok(())
    }
    fn reg_delete(&self, key: &str, name: &str) -> Result<(), OsError> {
        self.s().reg.remove(&(key.into(), name.into()));
        Ok(())
    }
    fn startup_items(&self) -> Result<Vec<StartupItem>, OsError> {
        Ok(self.s().startup.clone())
    }
    fn primary_display(&self) -> Result<String, OsError> {
        Ok(r"\\.\DISPLAY1".into())
    }
    fn display_mode(&self, _: &str) -> Result<DisplayMode, OsError> {
        self.s().current.ok_or_else(|| os("EnumDisplaySettingsW"))
    }
    fn display_modes(&self, _: &str) -> Result<Vec<DisplayMode>, OsError> {
        Ok(self.s().modes.clone())
    }
    fn set_display_hz(&self, _: &str, hz: u32) -> Result<(), OsError> {
        let mut s = self.s();
        let cur = s.current.ok_or_else(|| os("ChangeDisplaySettingsExW"))?;
        if !s
            .modes
            .iter()
            .any(|m| m.hz == hz && m.width == cur.width && m.height == cur.height)
        {
            return Err(os("ChangeDisplaySettingsExW"));
        }
        s.current = Some(DisplayMode { hz, ..cur });
        Ok(())
    }
}

#[derive(Default)]
pub struct MemJournal {
    rows: Mutex<Vec<JournalRow>>,
    seen: Mutex<HashMap<i64, Vec<&'static str>>>,
}

impl MemJournal {
    pub fn states_seen(&self, id: i64) -> Vec<&'static str> {
        self.seen
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }
}

impl JournalStore for MemJournal {
    fn journal_insert(&self, row: &NewJournalRow) -> Result<i64, String> {
        let mut rows = self.rows.lock().unwrap();
        let id = rows.len() as i64 + 1;
        rows.push(JournalRow {
            id,
            row: row.clone(),
            state: JournalState::Pending,
            created_utc: "now".into(),
            applied_utc: None,
            reverted_utc: None,
            error: None,
        });
        self.seen
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .push("pending");
        Ok(id)
    }
    fn journal_set_state(
        &self,
        id: i64,
        state: JournalState,
        error: Option<&str>,
    ) -> Result<(), String> {
        let mut rows = self.rows.lock().unwrap();
        let r = rows.iter_mut().find(|r| r.id == id).ok_or("no row")?;
        r.state = state;
        r.error = error.map(str::to_string);
        self.seen
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .push(state.as_str());
        Ok(())
    }
    fn journal_get(&self, id: i64) -> Result<Option<JournalRow>, String> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.id == id)
            .cloned())
    }
    fn journal_list(&self, f: &JournalFilter) -> Result<Vec<JournalRow>, String> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|r| f.txn_id.is_none_or(|t| r.row.txn_id == t))
            .filter(|r| f.scope.is_none_or(|s| r.row.scope == s))
            .filter(|r| f.executor.is_none_or(|e| r.row.executor == e))
            .filter(|r| f.owner.is_none_or(|o| r.row.owner == o))
            .filter(|r| !f.open_only || r.state.open())
            .cloned()
            .collect())
    }
}
