//! ICMP helpers via the IP Helper API (`IcmpSendEcho`) and default-gateway
//! discovery (`GetIpForwardTable`). No raw sockets / admin elevation needed.

use std::net::Ipv4Addr;
use std::sync::Once;

use windows::core::PCSTR;
use windows::Win32::Networking::WinSock::{WSAStartup, WSADATA};
use windows::Win32::NetworkManagement::IpHelper::{
    GetIpForwardTable, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, MIB_IPFORWARDTABLE,
};

static WSA_INIT: Once = Once::new();

fn ensure_wsa() {
    WSA_INIT.call_once(|| unsafe {
        let mut data = WSADATA::default();
        let _ = WSAStartup(0x0202, &mut data);
    });
}

/// Find the next-hop of the default IPv4 route (0.0.0.0/0).
pub fn default_gateway() -> Option<Ipv4Addr> {
    ensure_wsa();
    unsafe {
        let mut size = 0u32;
        let _ = GetIpForwardTable(None, &mut size, false);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let table = buf.as_mut_ptr() as *mut MIB_IPFORWARDTABLE;
        let rc = GetIpForwardTable(Some(table), &mut size, false);
        if rc != 0 {
            return None;
        }
        let table = &*table;
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
        rows.iter()
            .find(|r| r.dwForwardDest == 0 && r.dwForwardMask == 0 && r.dwForwardNextHop != 0)
            .map(|r| Ipv4Addr::from(u32::from_be(r.dwForwardNextHop)))
    }
}

/// Send one ICMP echo; returns round-trip milliseconds on success.
pub fn ping_once(target: Ipv4Addr, timeout_ms: u32) -> Option<u32> {
    ensure_wsa();
    unsafe {
        let Ok(handle) = IcmpCreateFile() else {
            return None;
        };
        let request: [u8; 32] = *b"networkclient-diagnostic-ping!!!";
        #[repr(C)]
        struct EchoReply {
            _addr: u32,
            status: u32,
            rtt: u32,
            _data_size: u16,
            _reserved: u16,
            _data_ptr: *mut core::ffi::c_void,
            _options: [u8; 8],
            data: [u8; 32],
        }
        let mut reply = std::mem::MaybeUninit::<EchoReply>::uninit();
        let reply_len = std::mem::size_of::<EchoReply>() as u32;
        let sent = IcmpSendEcho(
            handle,
            u32::from(target).to_be(),
            request.as_ptr() as *const core::ffi::c_void,
            request.len() as u16,
            None,
            reply.as_mut_ptr() as *mut core::ffi::c_void,
            reply_len,
            timeout_ms,
        );
        let _ = IcmpCloseHandle(handle);
        if sent == 0 {
            return None;
        }
        let reply = reply.assume_init();
        // IP_ECHO_REPLY_STATUS 0 = IP_SUCCESS.
        if reply.status != 0 {
            return None;
        }
        Some(reply.rtt)
    }
}

/// Run `count` pings, returning (success_rate 0..1, average latency ms).
pub fn ping_loss(target: Ipv4Addr, count: u32, timeout_ms: u32) -> (f64, Option<u32>) {
    let mut ok = 0u32;
    let mut total = 0u32;
    for _ in 0..count {
        if let Some(rtt) = ping_once(target, timeout_ms) {
            ok += 1;
            total += rtt;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let rate = ok as f64 / count as f64;
    let avg = (ok > 0).then(|| total / ok);
    (rate, avg)
}

#[allow(dead_code)]
fn pcstr_anchor() -> PCSTR {
    PCSTR::null()
}
