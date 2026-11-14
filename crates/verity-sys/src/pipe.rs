use std::ffi::c_void;
use std::io;

use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, ServerOptions};
use windows::core::PCWSTR;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_MODE, FILE_WRITE_DATA, OPEN_EXISTING,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};

use crate::security::SecurityDescriptor;
use crate::wide;

/// Opens the client end asking only for read + FILE_WRITE_DATA. GENERIC_WRITE
/// would also ask for FILE_CREATE_PIPE_INSTANCE, which the pipe's DACL refuses
/// to standard users. The server may identify the caller but not act as them.
/// Must be called inside a tokio runtime.
pub fn open_client(name: &str) -> io::Result<NamedPipeClient> {
    let w = wide(name);
    // SAFETY: `w` is NUL-terminated and outlives the call.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            GENERIC_READ.0 | FILE_WRITE_DATA.0,
            FILE_SHARE_MODE(0),
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            None,
        )
    }
    // Keep the plain Win32 code (low 16 bits of the HRESULT) so callers can
    // match ERROR_FILE_NOT_FOUND and ERROR_PIPE_BUSY.
    .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
    // SAFETY: `handle` is a valid pipe handle opened with FILE_FLAG_OVERLAPPED,
    // and ownership passes to the returned client, which closes it.
    unsafe { NamedPipeClient::from_raw_handle(handle.0) }
}

/// SYSTEM and Administrators: full control. Interactive users: read plus
/// FILE_WRITE_DATA only (0x0012008B = FILE_GENERIC_READ | FILE_WRITE_DATA).
/// Plain GW would include FILE_APPEND_DATA, which on a pipe is
/// FILE_CREATE_PIPE_INSTANCE and would let a user squat server instances.
/// The owner (OW) is whoever created the pipe: SYSTEM in production, the test
/// user in tests, which lets a non-admin test server create further instances.
const PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)(A;;0x0012008B;;;IU)";

/// Creates one server instance of the service pipe with Verity's DACL.
/// `first` must be true for the first instance so a pre-existing pipe of the
/// same name (a squatter) makes creation fail instead of being joined.
pub fn create_server(name: &str, first: bool) -> io::Result<NamedPipeServer> {
    let sd = SecurityDescriptor::from_sddl(PIPE_SDDL)?;
    let mut sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.as_ptr(),
        bInheritHandle: false.into(),
    };
    // SAFETY: `sa` points to a valid descriptor (`sd`) that lives past the call;
    // CreateNamedPipeW copies what it needs.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(name, &mut sa as *mut _ as *mut c_void)
    }
}
