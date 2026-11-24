//! Read-only WMI queries and one method call, for drive health and Optimize
//! Drives (docs/14-storage.md) and, later, Defender status (docs/15).
//! Namespaces, queries and method names are constants in the callers, never
//! client input.

use verity_core::OsError;
use windows::core::{BSTR, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_BSTR};
use windows::Win32::System::Wmi::{
    IEnumWbemClassObject, IWbemClassObject, IWbemLocator, IWbemServices, WbemLocator,
    WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_GENERIC_FLAG_TYPE, WBEM_INFINITE,
};

use crate::{hresult, wide};

/// A WMI property value, in the few shapes Verity reads.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Text(String),
}

impl Value {
    /// Integers, including CIM uint64 values, which WMI sends as strings.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Int(i) => u64::try_from(*i).ok(),
            Value::Text(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// One result row: the requested properties, in order.
pub type Row = Vec<Value>;

/// A connection to one WMI namespace, with COM initialized on this thread
/// for as long as it lives. Create, use and drop it on one thread.
pub struct Wmi {
    svc: IWbemServices,
    _com: Com,
}

struct Com;

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx in `Wmi::connect`.
        unsafe { CoUninitialize() };
    }
}

fn read(obj: &IWbemClassObject, name: &str) -> Value {
    let w = wide(name);
    let mut v = VARIANT::default();
    // SAFETY: `w` is NUL-terminated; `v` receives a copy that VARIANT's Drop clears.
    if unsafe { obj.Get(PCWSTR(w.as_ptr()), 0, &mut v, None, None) }.is_err() || v.is_empty() {
        return Value::Null;
    }
    if v.vt() == VT_BOOL {
        return bool::try_from(&v).map_or(Value::Null, Value::Bool);
    }
    if v.vt() == VT_BSTR {
        return BSTR::try_from(&v).map_or(Value::Null, |s| Value::Text(s.to_string()));
    }
    i64::try_from(&v).map_or(Value::Null, Value::Int)
}

impl Wmi {
    /// Connects to `namespace` (for example `ROOT\Microsoft\Windows\Storage`).
    pub fn connect(namespace: &str) -> Result<Self, OsError> {
        // SAFETY: no preconditions; balanced by `Com`'s Drop (also on S_FALSE).
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(hresult("CoInitializeEx"))?;
        let com = Com;
        // SAFETY: COM is initialized on this thread.
        let locator: IWbemLocator =
            unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER) }
                .map_err(hresult("CoCreateInstance(WbemLocator)"))?;
        // SAFETY: `locator` is live; the namespace BSTR outlives the call.
        let svc = unsafe {
            locator.ConnectServer(
                &BSTR::from(namespace),
                &BSTR::new(),
                &BSTR::new(),
                &BSTR::new(),
                0,
                &BSTR::new(),
                None,
            )
        }
        .map_err(hresult("IWbemLocator::ConnectServer"))?;
        // SAFETY: `svc` is a live proxy; these are the documented values for
        // local WMI calls made with the caller's own identity.
        unsafe {
            CoSetProxyBlanket(
                &svc,
                RPC_C_AUTHN_WINNT,
                RPC_C_AUTHZ_NONE,
                None,
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )
        }
        .map_err(hresult("CoSetProxyBlanket"))?;
        Ok(Self { svc, _com: com })
    }

    /// Runs a WQL query and returns `props` of every row, in order.
    pub fn query(&self, wql: &str, props: &[&str]) -> Result<Vec<Row>, OsError> {
        // SAFETY: `svc` is live; the BSTRs outlive the call.
        let rows: IEnumWbemClassObject = unsafe {
            self.svc.ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from(wql),
                WBEM_GENERIC_FLAG_TYPE(WBEM_FLAG_FORWARD_ONLY.0 | WBEM_FLAG_RETURN_IMMEDIATELY.0),
                None,
            )
        }
        .map_err(hresult("IWbemServices::ExecQuery"))?;
        let mut out = Vec::new();
        loop {
            let mut obj = [None; 1];
            let mut got = 0u32;
            // SAFETY: `obj` has room for one object; `got` receives the count.
            let hr = unsafe { rows.Next(WBEM_INFINITE, &mut obj, &mut got) };
            if hr.is_err() {
                return Err(OsError {
                    api: "IEnumWbemClassObject::Next",
                    code: hr.0 as u32,
                });
            }
            let Some(o) = obj[0].take().filter(|_| got == 1) else {
                break;
            };
            out.push(props.iter().map(|p| read(&o, p)).collect());
        }
        Ok(out)
    }

    /// Calls `method` of `class` on the object at `path` (its `__RELPATH`) with
    /// boolean arguments, and returns its `ReturnValue`. Arguments not given keep
    /// the method's defaults.
    pub fn call(
        &self,
        class: &str,
        path: &str,
        method: &str,
        args: &[(&str, bool)],
    ) -> Result<u64, OsError> {
        let mut cls: Option<IWbemClassObject> = None;
        // SAFETY: `svc` is live; the BSTR outlives the call; `cls` receives the class.
        unsafe {
            self.svc.GetObject(
                &BSTR::from(class),
                Default::default(),
                None,
                Some(&mut cls),
                None,
            )
        }
        .map_err(hresult("IWbemServices::GetObject"))?;
        let cls = cls.ok_or(OsError {
            api: "IWbemServices::GetObject",
            code: 0,
        })?;
        let name = wide(method);
        let mut signature: Option<IWbemClassObject> = None;
        // SAFETY: `name` is NUL-terminated; only the input signature is requested.
        unsafe {
            cls.GetMethod(
                PCWSTR(name.as_ptr()),
                0,
                &mut signature,
                std::ptr::null_mut(),
            )
        }
        .map_err(hresult("IWbemClassObject::GetMethod"))?;
        let inputs = match signature {
            Some(sig) => {
                // SAFETY: `sig` is the method's input signature class.
                let inst = unsafe { sig.SpawnInstance(0) }
                    .map_err(hresult("IWbemClassObject::SpawnInstance"))?;
                for (arg, on) in args {
                    let key = wide(arg);
                    let v = VARIANT::from(*on);
                    // SAFETY: `key` is NUL-terminated; `v` outlives the call; type 0
                    // means the property's declared type.
                    unsafe { inst.Put(PCWSTR(key.as_ptr()), 0, &v, 0) }
                        .map_err(hresult("IWbemClassObject::Put"))?;
                }
                Some(inst)
            }
            None => None,
        };
        let mut result: Option<IWbemClassObject> = None;
        // SAFETY: `svc` is live; the BSTRs outlive the call; `result` receives the out-parameters.
        unsafe {
            self.svc.ExecMethod(
                &BSTR::from(path),
                &BSTR::from(method),
                Default::default(),
                None,
                inputs.as_ref(),
                Some(&mut result),
                None,
            )
        }
        .map_err(hresult("IWbemServices::ExecMethod"))?;
        Ok(result
            .map(|r| read(&r, "ReturnValue"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_standard_class() {
        let w = Wmi::connect(r"ROOT\CIMV2").unwrap();
        let rows = w
            .query(
                "SELECT Caption, BuildNumber, OSArchitecture FROM Win32_OperatingSystem",
                &["Caption", "BuildNumber", "Missing"],
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0][0].as_str().unwrap().contains("Windows"));
        assert!(rows[0][1].as_u64().unwrap() >= 19_045);
        assert_eq!(rows[0][2], Value::Null);
    }
}
