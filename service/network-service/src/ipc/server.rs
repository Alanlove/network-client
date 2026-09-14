//! Named Pipe IPC server (design doc §6–§9). One pipe instance per UI
//! connection; each connection runs a reader/dispatcher loop, a single
//! serialized writer and an event-forwarding task.
//!
//! The DACL grants access to the pipe owner (the service user) only.

use std::sync::Arc;

use futures::FutureExt;
use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::mpsc;

use crate::app::context::AppContext;
use crate::ipc::frame::MAX_FRAME;
use crate::ipc::proto::{Event, Request, Response};

pub const PIPE_NAME: &str = r"\\.\pipe\NetworkClient";

pub async fn run(ctx: Arc<AppContext>, shutdown: tokio::sync::watch::Receiver<bool>) -> std::io::Result<()> {
    let mut first = true;
    loop {
        let server = create_instance(first)?;
        first = false;
        tokio::select! {
            r = server.connect() => r?,
            _ = wait_shutdown(shutdown.clone()) => return Ok(()),
        }
        let ctx = ctx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(server, ctx).await {
                tracing::debug!("pipe connection ended: {e}");
            }
        });
    }
}

async fn wait_shutdown(mut rx: tokio::sync::watch::Receiver<bool>) {
    let _ = rx.changed().await;
}

#[cfg(windows)]
fn create_instance(first: bool) -> std::io::Result<NamedPipeServer> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    // Owner-only DACL: the user running the service is the only client.
    let sddl: Vec<u16> = "D:P(A;;GA;;;OW)\0".encode_utf16().collect();
    let mut psd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut psd,
            None,
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: psd.0,
        bInheritHandle: false.into(),
    };

    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first);
    let result = unsafe { opts.create_with_security_attributes_raw(PIPE_NAME, &sa as *const _ as *mut _) };
    unsafe {
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(psd.0));
    }
    result
}

#[cfg(not(windows))]
fn create_instance(_first: bool) -> std::io::Result<NamedPipeServer> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "named pipes are Windows-only",
    ))
}

async fn handle_connection(
    pipe: NamedPipeServer,
    ctx: Arc<AppContext>,
) -> std::io::Result<()> {
    let (mut reader, mut writer) = tokio::io::split(pipe);

    // Single writer fed by both responses and event pushes.
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
    let writer_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if writer.write_all(&frame).await.is_err() {
                break;
            }
            let _ = writer.flush().await;
        }
    });

    // Event forwarder.
    let mut events = ctx.bus.subscribe();
    let evt_tx = tx.clone();
    let event_task = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(msg) => {
                    let frame = wrap(&Event {
                        event_type: msg.event_type,
                        payload: msg.payload,
                        timestamp: msg.timestamp,
                    });
                    if evt_tx.send(frame).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    // Request loop.
    loop {
        let mut len_buf = [0u8; 4];
        if reader.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > MAX_FRAME {
            break;
        }
        let mut buf = vec![0u8; len];
        if reader.read_exact(&mut buf).await.is_err() {
            break;
        }
        let req = match Request::decode(buf.as_slice()) {
            Ok(r) => r,
            Err(e) => {
                let frame = wrap(&Response {
                    request_id: String::new(),
                    code: 422,
                    message: format!("bad request: {e}"),
                    payload: Vec::new(),
                });
                if tx.send(frame).await.is_err() {
                    break;
                }
                continue;
            }
        };

        let ctx = ctx.clone();
        let reply_tx = tx.clone();
        tokio::spawn(async move {
            // Panic isolation: a handler bug (unwrap/index/etc.) must kill
            // only this request task — still reply a 500 so the client can
            // react instead of hanging forever on a missing frame.
            let dispatched = std::panic::AssertUnwindSafe(ctx.dispatch(&req.method, &req.payload))
                .catch_unwind()
                .await;
            let response = match dispatched {
                Ok(Ok(payload)) => Response {
                    request_id: req.request_id,
                    code: 0,
                    message: "ok".into(),
                    payload,
                },
                Ok(Err(e)) => {
                    tracing::debug!("method {} failed: {e}", req.method);
                    Response {
                        request_id: req.request_id,
                        code: e.code(),
                        message: e.to_string(),
                        payload: Vec::new(),
                    }
                }
                Err(panic) => {
                    let detail = panic
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "handler panicked".into());
                    tracing::error!("method {} panicked: {detail}", req.method);
                    Response {
                        request_id: req.request_id,
                        code: 500,
                        message: format!("internal handler panic: {detail}"),
                        payload: Vec::new(),
                    }
                }
            };
            let _ = reply_tx.send(wrap(&response)).await;
        });
    }

    event_task.abort();
    drop(tx);
    writer_task.abort();
    Ok(())
}

fn wrap<M: Message>(msg: &M) -> Vec<u8> {
    let body = msg.encode_to_vec();
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    frame
}
