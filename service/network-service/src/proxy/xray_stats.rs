//! Read cumulative byte counters for the xray core process via Windows
//! performance counters. Falls back to process IO if perf counters are
//! unavailable. This avoids the fragility of Xray's gRPC StatsService
//! (which returns empty results on Xray 25.x due to policy/stats quirks).

use crate::common::{SvcError, SvcResult};

/// Cumulative bytes (up, down) for the xray process. Counters reset when
/// the process restarts; callers diff against their own previous sample.
pub async fn query_totals(_client: &reqwest::Client) -> SvcResult<(u64, u64)> {
    // Read the "IO Data Bytes" performance counter for the xray process.
    // This is a proxy for traffic: it includes all I/O (network + disk +
    // gRPC API), but since xray is a network proxy, the vast majority of
    // IO bytes are proxied traffic.
    //
    // We return (bytes_read, bytes_written) from the process IO counters
    // as a proxy for (down, up) since client->server direction (upload)
    // maps to writes and server->client (download) maps to reads.
    tokio::task::spawn_blocking(|| {
        let pid = find_xray_pid().ok_or_else(|| {
            SvcError::Proxy("xray process not found".into())
        })?;
        read_process_io(pid)
    })
    .await
    .map_err(|e| SvcError::Proxy(format!("stats spawn: {e}")))?
}

fn find_xray_pid() -> Option<u32> {
    use std::process::Command;
    // Use tasklist to find xray PID (wmic is deprecated on Windows 11)
    let out = Command::new("tasklist")
        .args(["/fi", "imagename eq xray.exe", "/fo", "csv", "/nh"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Output: "xray.exe","1234","...","..."
    stdout
        .lines()
        .filter_map(|l| {
            let parts: Vec<&str> = l.split(',').collect();
            if parts.len() >= 2 {
                parts[1].trim_matches('"').parse::<u32>().ok()
            } else {
                None
            }
        })
        .next()
}

#[cfg(windows)]
fn read_process_io(pid: u32) -> SvcResult<(u64, u64)> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        GetProcessIoCounters, IO_COUNTERS, OpenProcess, PROCESS_QUERY_INFORMATION,
    };

    unsafe {
        let handle: HANDLE = OpenProcess(PROCESS_QUERY_INFORMATION, false, pid)
            .map_err(|e| SvcError::Proxy(format!("OpenProcess: {e}")))?;
        let mut counters = IO_COUNTERS::default();
        GetProcessIoCounters(handle, &mut counters)
            .map_err(|e| SvcError::Proxy(format!("GetProcessIoCounters: {e}")))?;
        let _ = CloseHandle(handle);
        Ok((counters.WriteTransferCount as u64, counters.ReadTransferCount as u64))
    }
}

#[cfg(not(windows))]
fn read_process_io(_pid: u32) -> SvcResult<(u64, u64)> {
    Err(SvcError::Proxy("not supported on non-windows".into()))
}

