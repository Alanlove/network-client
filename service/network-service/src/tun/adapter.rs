//! Runtime-loaded Wintun bindings (design doc §24). The DLL is not shipped;
//! loading failures degrade gracefully to System Proxy mode.
//!
//! Wintun is a single `wintun.dll` placed next to the service or under
//! `bin/`. Symbols are resolved with `libloading` so the service builds and
//! runs without the SDK present.

#![allow(non_camel_case_types, non_snake_case)]

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use libloading::Library;

use crate::common::{SvcError, SvcResult};

type WINTUN_ADAPTER_HANDLE = *mut c_void;
type WINTUN_SESSION_HANDLE = *mut c_void;

#[repr(C)]
struct WintunGuid {
    data: [u8; 16],
}

type FnCreateAdapter =
    unsafe extern "C" fn(*const u16, *const u16, *const WintunGuid) -> WINTUN_ADAPTER_HANDLE;
type FnOpenAdapter = unsafe extern "C" fn(*const u16) -> WINTUN_ADAPTER_HANDLE;
type FnCloseAdapter = unsafe extern "C" fn(WINTUN_ADAPTER_HANDLE) -> u32;
type FnStartSession = unsafe extern "C" fn(WINTUN_ADAPTER_HANDLE, u32) -> WINTUN_SESSION_HANDLE;
type FnEndSession = unsafe extern "C" fn(WINTUN_SESSION_HANDLE);
type FnDriverVersion = unsafe extern "C" fn() -> u32;

pub(crate) struct WintunLib {
    _lib: Library,
    create_adapter: FnCreateAdapter,
    open_adapter: FnOpenAdapter,
    close_adapter: FnCloseAdapter,
    start_session: FnStartSession,
    end_session: FnEndSession,
    driver_version: FnDriverVersion,
}

unsafe impl Send for WintunLib {}
unsafe impl Sync for WintunLib {}

pub struct AdapterHandle {
    handle: WINTUN_ADAPTER_HANDLE,
    close: FnCloseAdapter,
    end_session: FnEndSession,
    session: WINTUN_SESSION_HANDLE,
}

unsafe impl Send for AdapterHandle {}
unsafe impl Sync for AdapterHandle {}

fn wide(name: &str) -> Vec<u16> {
    Path::new(name)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

impl WintunLib {
    pub fn load(dir: &Path) -> SvcResult<Self> {
        let dll = dir.join("wintun.dll");
        // SAFETY: loading a user-supplied DLL from the app data dir; this is
        // the documented operator-provided integration point.
        let lib = unsafe {
            Library::new(&dll).map_err(|e| {
                SvcError::Tun(format!(
                    "failed to load {} (TUN mode unavailable): {e}",
                    dll.display()
                ))
            })?
        };
        unsafe {
            Ok(Self {
                create_adapter: resolve(&lib, b"WintunCreateAdapter\0")?,
                open_adapter: resolve(&lib, b"WintunOpenAdapter\0")?,
                close_adapter: resolve(&lib, b"WintunCloseAdapter\0")?,
                start_session: resolve(&lib, b"WintunStartSession\0")?,
                end_session: resolve(&lib, b"WintunEndSession\0")?,
                driver_version: resolve(&lib, b"WintunGetRunningDriverVersion\0")?,
                _lib: lib,
            })
        }
    }

    pub fn driver_version(&self) -> u32 {
        unsafe { (self.driver_version)() }
    }

    /// Open or create the adapter, then start a 4 MiB ring session.
    pub fn start(&self, name: &str) -> SvcResult<AdapterHandle> {
        let wname = wide(name);
        unsafe {
            let mut adapter = (self.open_adapter)(wname.as_ptr());
            if adapter.is_null() {
                let wtype = wide("NetworkClient");
                adapter = (self.create_adapter)(wname.as_ptr(), wtype.as_ptr(), std::ptr::null());
            }
            if adapter.is_null() {
                return Err(SvcError::Tun(format!(
                    "WintunCreateAdapter(\"{name}\") failed (code {})",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
                )));
            }
            // 4 MiB ring capacity.
            const CAPACITY: u32 = 0x0040_0000;
            let session = (self.start_session)(adapter, CAPACITY);
            if session.is_null() {
                (self.close_adapter)(adapter);
                return Err(SvcError::Tun("WintunStartSession failed".into()));
            }
            Ok(AdapterHandle {
                handle: adapter,
                close: self.close_adapter,
                end_session: self.end_session,
                session,
            })
        }
    }
}

unsafe fn resolve<T: Copy>(lib: &Library, name: &[u8]) -> SvcResult<T> {
    let cname = std::ffi::CStr::from_bytes_with_nul(name).expect("cstr");
    lib.get::<T>(cname.to_bytes_with_nul())
        .map(|s| *s)
        .map_err(|e| SvcError::Tun(format!("missing wintun symbol {}: {e}", cname.to_string_lossy())))
}

impl Drop for AdapterHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.session.is_null() {
                (self.end_session)(self.session);
            }
            if !self.handle.is_null() {
                (self.close)(self.handle);
            }
        }
    }
}
