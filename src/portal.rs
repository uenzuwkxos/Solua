// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux/Wayland: global shortcuts through the `org.freedesktop.portal.GlobalShortcuts` portal (KDE Plasma,
//! GNOME 48+…).
//!
//! On Wayland an application cannot capture keys by itself: it registers its commands
//! and the desktop assigns the keys (the user changes them in System Settings).

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::MatchRule;

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

/// A command the application offers to the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub id: String,
    pub description: String,
    /// suggested key, in the portal format (e.g. "CTRL+ALT+Up"); the desktop may change it
    pub trigger: String,
}

#[derive(Clone, Debug)]
pub enum Event {
    /// the user triggered the shortcut
    Activated(String),
    /// registered shortcuts: (id, description of the assigned key)
    Bound(Vec<(String, String)>),
}

pub type Callback = Arc<dyn Fn(Event) + Send + Sync>;

/// Starts the portal thread. Each list sent through the channel replaces the registered shortcuts.
pub fn start(cb: Callback) -> Sender<Vec<Shortcut>> {
    let (tx, rx) = channel::<Vec<Shortcut>>();
    let _ = std::thread::Builder::new().name("solua-portal".into()).spawn(move || {
        let conn = match Connection::session() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("solua: no session bus ({e})");
                return;
            }
        };
        // Tell the portal the app_id (needed when the program was not launched from the menu).
        let _ = conn.call_method(
            Some(DEST),
            PATH,
            Some("org.freedesktop.host.portal.Registry"),
            "Register",
            &("solua", HashMap::<&str, Value>::new()),
        );
        spawn_listener(conn.clone(), cb.clone());
        let mut current: Vec<Shortcut> = Vec::new();
        let mut session: Option<String> = None;
        let mut counter = 0u32;
        while let Ok(mut list) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                list = newer; // only the most recent list matters
            }
            if list == current && session.is_some() {
                continue;
            }
            if let Some(s) = session.take() {
                let _ = conn.call_method(Some(DEST), s.as_str(), Some("org.freedesktop.portal.Session"), "Close", &());
            }
            current = list.clone();
            if list.is_empty() {
                continue;
            }
            counter += 1;
            match bind(&conn, &list, counter) {
                Ok((path, bound)) => {
                    session = Some(path);
                    cb(Event::Bound(bound));
                }
                Err(e) => eprintln!("solua: global shortcuts unavailable ({e})"),
            }
        }
    });
    tx
}

fn spawn_listener(conn: Connection, cb: Callback) {
    let _ = std::thread::Builder::new().name("solua-portal-signals".into()).spawn(move || {
        let Ok(rule) = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(IFACE)
            .and_then(|b| b.member("Activated"))
            .map(|b| b.build())
        else {
            return;
        };
        let Ok(it) = MessageIterator::for_match_rule(rule, &conn, Some(16)) else { return };
        for msg in it.flatten() {
            if let Ok((_session, id, _ts, _opts)) = msg.body().deserialize::<(ObjectPath, String, u64, HashMap<String, OwnedValue>)>() {
                cb(Event::Activated(id));
            }
        }
    });
}

/// Calls a portal method that answers asynchronously (`Request`) and waits for the response.
fn request(
    conn: &Connection,
    method: &str,
    token: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> Result<HashMap<String, OwnedValue>, String> {
    let sender = conn.unique_name().map(|n| n.as_str().trim_start_matches(':').replace('.', "_")).ok_or("no bus name")?;
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")
        .and_then(|b| b.member("Response"))
        .and_then(|b| b.path(path.as_str()))
        .map(|b| b.build())
        .map_err(|e| e.to_string())?;
    let mut it = MessageIterator::for_match_rule(rule, conn, Some(4)).map_err(|e| e.to_string())?;
    conn.call_method(Some(DEST), PATH, Some(IFACE), method, body).map_err(|e| e.to_string())?;
    let msg = it.next().ok_or("no response")?.map_err(|e| e.to_string())?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize().map_err(|e| e.to_string())?;
    if code != 0 {
        return Err(format!("{method} refused (code {code})"));
    }
    Ok(results)
}

fn bind(conn: &Connection, list: &[Shortcut], n: u32) -> Result<(String, Vec<(String, String)>), String> {
    let sender = conn.unique_name().map(|n| n.as_str().trim_start_matches(':').replace('.', "_")).ok_or("no bus name")?;
    let stoken = format!("solua_s{}_{n}", std::process::id());
    let mut opts: HashMap<&str, Value> = HashMap::new();
    opts.insert("handle_token", Value::from(format!("solua_c{}_{n}", std::process::id())));
    opts.insert("session_handle_token", Value::from(stoken.clone()));
    request(conn, "CreateSession", &format!("solua_c{}_{n}", std::process::id()), &(opts,))?;
    let session = format!("/org/freedesktop/portal/desktop/session/{sender}/{stoken}");

    let shortcuts: Vec<(String, HashMap<&str, Value>)> = list
        .iter()
        .map(|s| {
            let mut m: HashMap<&str, Value> = HashMap::new();
            m.insert("description", Value::from(s.description.clone()));
            if !s.trigger.is_empty() {
                m.insert("preferred_trigger", Value::from(s.trigger.clone()));
            }
            (s.id.clone(), m)
        })
        .collect();
    let btoken = format!("solua_b{}_{n}", std::process::id());
    let mut bopts: HashMap<&str, Value> = HashMap::new();
    bopts.insert("handle_token", Value::from(btoken.clone()));
    let sess = ObjectPath::try_from(session.as_str()).map_err(|e| e.to_string())?;
    let res = request(conn, "BindShortcuts", &btoken, &(sess, shortcuts, "", bopts))?;
    let mut out = Vec::new();
    if let Some(v) = res.get("shortcuts") {
        if let Ok(list) = v.try_clone().map_err(|e| e.to_string()).and_then(|v| {
            v.try_into().map_err(|e: zbus::zvariant::Error| e.to_string())
        }) as Result<Vec<(String, HashMap<String, OwnedValue>)>, String>
        {
            for (id, props) in list {
                let desc = props
                    .get("trigger_description")
                    .and_then(|d| <&str>::try_from(&**d).ok().map(str::to_string))
                    .unwrap_or_default();
                out.push((id, desc));
            }
        }
    }
    if out.is_empty() {
        out = list.iter().map(|s| (s.id.clone(), String::new())).collect();
    }
    Ok((session, out))
}
