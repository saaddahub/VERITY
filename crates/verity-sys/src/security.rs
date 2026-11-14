use std::ffi::c_void;

use verity_core::OsError;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{GetSecurityDescriptorDacl, ACL, PSECURITY_DESCRIPTOR};

use crate::{hresult, wide};

/// A security descriptor built from an SDDL string, freed on drop.
pub(crate) struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    pub(crate) fn from_sddl(sddl: &str) -> Result<Self, OsError> {
        let w = wide(sddl);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `w` is NUL-terminated and outlives the call; `sd` receives a
        // LocalAlloc'd descriptor that Drop frees exactly once.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(w.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        }
        .map_err(hresult(
            "ConvertStringSecurityDescriptorToSecurityDescriptorW",
        ))?;
        Ok(Self(sd))
    }

    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.0 .0
    }

    /// The descriptor's DACL; valid while `self` lives.
    pub(crate) fn dacl(&self) -> Result<*mut ACL, OsError> {
        let mut present = Default::default();
        let mut defaulted = Default::default();
        let mut dacl = std::ptr::null_mut();
        // SAFETY: `self.0` is a valid descriptor; out-pointers are valid locals.
        unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted) }
            .map_err(hresult("GetSecurityDescriptorDacl"))?;
        Ok(dacl)
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: allocated with LocalAlloc by the SDDL converter, freed once here.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}
