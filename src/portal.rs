use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::fd::{IntoRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MsgType;
use zbus::zvariant::{ObjectPath, OwnedFd as ZvOwnedFd, OwnedObjectPath, OwnedValue, Value};
use zbus::MatchRule;

#[derive(Debug)]
pub struct PortalSelection {
    pub node_id: u32,
    pub pw_fd: RawFd,
    pub stream_position: Option<(i32, i32)>,
    pub stream_size: Option<(i32, i32)>,
    pub session: PortalSessionGuard,
}

#[derive(Debug)]
pub struct PortalSessionGuard {
    conn: Connection,
    session_path: String,
    closed_flag: Arc<AtomicBool>,
}

impl PortalSessionGuard {
    pub fn is_closed(&self) -> bool {
        self.closed_flag.load(Ordering::Relaxed)
    }
}

impl Drop for PortalSessionGuard {
    fn drop(&mut self) {
        if let Ok(path) = ObjectPath::try_from(self.session_path.as_str()) {
            let _ = self.conn.call_method(
                Some("org.freedesktop.portal.Desktop"),
                path,
                Some("org.freedesktop.portal.Session"),
                "Close",
                &(),
            );
        }
    }
}

fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("portal-screen-mirror");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".local/state/portal-screen-mirror");
        }
    }
    PathBuf::from("/tmp/portal-screen-mirror-state")
}

pub fn load_restore_tokens() -> HashMap<String, String> {
    let path = state_dir().join("restore_tokens.json");
    let Ok(raw) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str::<HashMap<String, String>>(&raw).unwrap_or_default()
}

pub fn save_restore_tokens(tokens: &HashMap<String, String>) {
    let dir = state_dir();
    let _ = fs::create_dir_all(&dir);
    let tmp = dir.join("restore_tokens.json.tmp");
    let dst = dir.join("restore_tokens.json");
    if let Ok(json) = serde_json::to_string_pretty(tokens) {
        if let Ok(mut f) = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
        {
            if f.write_all(json.as_bytes()).is_ok() {
                let _ = fs::rename(&tmp, &dst);
            }
        }
    }
}

fn sender_slug(conn: &Connection) -> Result<String, String> {
    let uniq = conn
        .unique_name()
        .ok_or_else(|| "D-Bus connection has no unique name".to_string())?;
    Ok(uniq.as_str().trim_start_matches(':').replace('.', "_"))
}

fn call_portal_request<B: serde::Serialize + zbus::zvariant::DynamicType>(
    conn: &Connection,
    sender: &str,
    seq: &mut u32,
    method: &str,
    body: &B,
    expected_token: &str,
) -> Result<(u32, HashMap<String, OwnedValue>), String> {
    *seq += 1;
    let expected_path =
        format!("/org/freedesktop/portal/desktop/request/{sender}/{expected_token}");

    let rule = MatchRule::builder()
        .msg_type(MsgType::Signal)
        .interface("org.freedesktop.portal.Request")
        .map_err(|e| e.to_string())?
        .member("Response")
        .map_err(|e| e.to_string())?
        .build();

    let mut iter = MessageIterator::for_match_rule(rule, conn, Some(64))
        .map_err(|e| format!("Failed subscribing to Portal Request::Response: {e}"))?;

    let reply = conn
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.ScreenCast"),
            method,
            body,
        )
        .map_err(|e| format!("Portal {method} D-Bus call failed: {e}"))?;

    let actual_path: OwnedObjectPath = reply
        .body()
        .deserialize()
        .map_err(|e| format!("Failed parsing {method} handle path: {e}"))?;
    let actual_str = actual_path.as_str().to_string();

    for msg_res in &mut iter {
        let msg = msg_res.map_err(|e| format!("D-Bus signal stream error: {e}"))?;
        let hdr = msg.header();
        let Some(hdr_path) = hdr.path() else {
            continue;
        };
        if hdr_path.as_str() == actual_str || hdr_path.as_str() == expected_path {
            let (code, results): (u32, HashMap<String, OwnedValue>) = msg
                .body()
                .deserialize()
                .map_err(|e| format!("Failed decoding Portal Response body: {e}"))?;
            return Ok((code, results));
        }
    }

    Err(format!("Portal request {method} ended without a Response"))
}

pub enum PortalCaptureKind {
    MonitorOrWindow,
    MonitorOnly { restore_key: Option<String> },
}

pub fn open_screencast_portal(
    kind: PortalCaptureKind,
    show_cursor: bool,
    wakeup_eventfd: RawFd,
) -> Result<Option<PortalSelection>, String> {
    let conn =
        Connection::session().map_err(|e| format!("Failed connecting to session D-Bus: {e}"))?;
    let sender = sender_slug(&conn)?;
    let pid = std::process::id();
    let mut seq = 0u32;

    let tok1 = format!("psm_req_{pid}_1");
    let sess_tok = format!("psm_sess_{pid}");
    let mut create_opts: HashMap<&str, Value<'_>> = HashMap::new();
    create_opts.insert("handle_token", Value::from(tok1.as_str()));
    create_opts.insert("session_handle_token", Value::from(sess_tok.as_str()));

    let (code, res) = call_portal_request(
        &conn,
        &sender,
        &mut seq,
        "CreateSession",
        &(create_opts,),
        &tok1,
    )?;
    if code == 1 {
        return Ok(None);
    }
    if code != 0 {
        return Err(format!("ScreenCast CreateSession failed (response={code})"));
    }

    let session_path = res
        .get("session_handle")
        .and_then(|v| String::try_from(v.clone()).ok())
        .or_else(|| {
            res.get("session_handle")
                .and_then(|v| ObjectPath::try_from(v.clone()).ok().map(|p| p.to_string()))
        })
        .ok_or_else(|| "Portal CreateSession returned no session_handle".to_string())?;

    let closed_flag = Arc::new(AtomicBool::new(false));
    {
        let conn_clone = conn.clone();
        let sess_clone = session_path.clone();
        let flag_clone = Arc::clone(&closed_flag);
        if let Ok(path_obj) = ObjectPath::try_from(sess_clone.as_str()) {
            if let Ok(rule) = MatchRule::builder()
                .msg_type(MsgType::Signal)
                .interface("org.freedesktop.portal.Session")
                .and_then(|b| b.member("Closed"))
                .and_then(|b| b.path(path_obj))
            {
                if let Ok(mut closed_iter) =
                    MessageIterator::for_match_rule(rule.build(), &conn_clone, Some(4))
                {
                    std::thread::spawn(move || {
                        if let Some(Ok(_)) = closed_iter.next() {
                            flag_clone.store(true, Ordering::Relaxed);
                            if wakeup_eventfd >= 0 {
                                let one: u64 = 1;
                                unsafe {
                                    let _ = libc::write(
                                        wakeup_eventfd,
                                        &one as *const u64 as *const libc::c_void,
                                        std::mem::size_of::<u64>(),
                                    );
                                }
                            }
                        }
                    });
                }
            }
        }
    }

    let session_guard = PortalSessionGuard {
        conn: conn.clone(),
        session_path: session_path.clone(),
        closed_flag,
    };

    let sess_obj = ObjectPath::try_from(session_path.as_str()).map_err(|e| e.to_string())?;
    let mut tokens = load_restore_tokens();

    let tok2 = format!("psm_req_{pid}_2");
    let mut select_opts: HashMap<&str, Value<'_>> = HashMap::new();
    select_opts.insert("handle_token", Value::from(tok2.as_str()));
    select_opts.insert("multiple", Value::from(false));
    select_opts.insert(
        "cursor_mode",
        Value::from(if show_cursor { 2u32 } else { 1u32 }),
    );

    let saved_token_owned: Option<String>;
    match &kind {
        PortalCaptureKind::MonitorOrWindow => {
            select_opts.insert("types", Value::from(3u32));
        }
        PortalCaptureKind::MonitorOnly { restore_key } => {
            select_opts.insert("types", Value::from(1u32));
            select_opts.insert("persist_mode", Value::from(2u32));
            saved_token_owned = restore_key.as_ref().and_then(|k| tokens.get(k).cloned());
            if let Some(ref rt) = saved_token_owned {
                select_opts.insert("restore_token", Value::from(rt.as_str()));
            }
        }
    }

    let (code, _) = call_portal_request(
        &conn,
        &sender,
        &mut seq,
        "SelectSources",
        &(&sess_obj, select_opts),
        &tok2,
    )?;
    if code == 1 {
        return Ok(None);
    }
    if code != 0 {
        return Err(format!("ScreenCast SelectSources failed (response={code})"));
    }

    let tok3 = format!("psm_req_{pid}_3");
    let mut start_opts: HashMap<&str, Value<'_>> = HashMap::new();
    start_opts.insert("handle_token", Value::from(tok3.as_str()));

    let (code, start_res) = call_portal_request(
        &conn,
        &sender,
        &mut seq,
        "Start",
        &(&sess_obj, "", start_opts),
        &tok3,
    )?;
    if code == 1 {
        return Ok(None);
    }
    if code != 0 {
        return Err(format!("ScreenCast Start failed (response={code})"));
    }

    let restore_token = start_res
        .get("restore_token")
        .and_then(|v| String::try_from(v.clone()).ok())
        .filter(|s| !s.is_empty());

    let streams_val = start_res
        .get("streams")
        .ok_or_else(|| "Portal Start returned no streams".to_string())?;
    let streams: Vec<(u32, HashMap<String, OwnedValue>)> = streams_val
        .clone()
        .try_into()
        .map_err(|e| format!("Failed parsing Portal streams array: {e}"))?;
    let (node_id, stream_props) = streams
        .into_iter()
        .next()
        .ok_or_else(|| "Portal Start returned empty streams list".to_string())?;

    let stream_position = stream_props
        .get("position")
        .and_then(|v| <(i32, i32)>::try_from(v.clone()).ok());
    let stream_size = stream_props
        .get("size")
        .and_then(|v| <(i32, i32)>::try_from(v.clone()).ok());

    if let (
        PortalCaptureKind::MonitorOnly {
            restore_key: Some(key),
        },
        Some(rt),
    ) = (&kind, &restore_token)
    {
        tokens.insert(key.clone(), rt.clone());
        save_restore_tokens(&tokens);
    }

    let empty_opts: HashMap<&str, Value<'_>> = HashMap::new();
    let fd_reply = conn
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.ScreenCast"),
            "OpenPipeWireRemote",
            &(&sess_obj, empty_opts),
        )
        .map_err(|e| format!("OpenPipeWireRemote failed: {e}"))?;

    let zv_fd: ZvOwnedFd = fd_reply
        .body()
        .deserialize()
        .map_err(|e| format!("Failed extracting PipeWire remote FD: {e}"))?;
    let owned_fd: OwnedFd = zv_fd.into();

    Ok(Some(PortalSelection {
        node_id,
        pw_fd: owned_fd.into_raw_fd(),
        stream_position,
        stream_size,
        session: session_guard,
    }))
}
