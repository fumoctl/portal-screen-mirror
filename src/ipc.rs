use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum FitMode {
    Contain,
    Cover,
    Stretch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum FilterMode {
    Linear,
    Nearest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcRequest {
    Status,
    Stop,
    Set {
        fit: Option<FitMode>,
        filter: Option<FilterMode>,
        max_fps: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorStatus {
    pub running: bool,
    pub pid: u32,
    pub mode: String,
    pub backend: String,
    pub destination: String,
    pub fit: FitMode,
    pub filter: FilterMode,
    pub max_fps_spec: String,
    pub effective_fps_cap: u32,
    pub source_width: u32,
    pub source_height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<MirrorStatus>,
}

pub fn socket_path() -> PathBuf {
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        if !rt.is_empty() {
            return PathBuf::from(rt).join("portal-screen-mirror.sock");
        }
    }
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(format!("/tmp/portal-screen-mirror-{uid}.sock"))
}

pub fn send_request(req: &IpcRequest) -> Result<IpcResponse, String> {
    send_request_timeout(req, Duration::from_millis(1500))
}

pub fn send_request_timeout(req: &IpcRequest, timeout: Duration) -> Result<IpcResponse, String> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path)
        .map_err(|_| "No active portal-screen-mirror session running".to_string())?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let mut payload = serde_json::to_string(req).map_err(|e| e.to_string())?;
    payload.push('\n');
    stream
        .write_all(payload.as_bytes())
        .map_err(|e| format!("Failed writing to IPC socket: {e}"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("Failed reading IPC response: {e}"))?;
    if line.trim().is_empty() {
        return Err("Empty response from running instance".to_string());
    }
    serde_json::from_str::<IpcResponse>(line.trim())
        .map_err(|e| format!("Malformed IPC response: {e}"))
}

pub struct IpcServer {
    path: PathBuf,
    listener: UnixListener,
}

impl IpcServer {
    pub fn bind() -> Result<Self, String> {
        let path = socket_path();
        if path.exists() {
            if send_request(&IpcRequest::Status).is_ok() {
                return Err(
                    "Another portal-screen-mirror instance is already running (use `stop` or `toggle`)"
                        .to_string(),
                );
            }
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path)
            .map_err(|e| format!("Failed to bind IPC socket {}: {e}", path.display()))?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Failed setting IPC listener nonblocking: {e}"))?;
        Ok(Self { path, listener })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.listener.as_raw_fd()
    }

    pub fn poll_requests<F>(&self, mut handler: F)
    where
        F: FnMut(IpcRequest) -> IpcResponse,
    {
        loop {
            let (mut stream, _) = match self.listener.accept() {
                Ok(pair) => pair,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            };
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));

            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut line).is_ok() && !line.trim().is_empty() {
                let resp = match serde_json::from_str::<IpcRequest>(line.trim()) {
                    Ok(req) => handler(req),
                    Err(e) => IpcResponse {
                        ok: false,
                        message: format!("Invalid IPC request: {e}"),
                        status: None,
                    },
                };
                if let Ok(mut out) = serde_json::to_string(&resp) {
                    out.push('\n');
                    let _ = stream.write_all(out.as_bytes());
                }
            }
        }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
