//! Feature: Spotify for the XENEON EDGE Spotify widget (Web API, the user's own Spotify app).
//! Snapshot `spotify/1`: track, artwork and lyrics shown together share one revision `rev`;
//! artwork and lyrics fetched for an older revision are dropped. Commands are an allowlist.
pub mod api;
pub mod auth;

use crate::util::{now_ms, SEC};
use serde_json::{json, Value};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};

pub const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const HEARTBEAT_MS: u64 = 5 * SEC;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    NotConfigured,
    Connecting,
    Connected,
    NeedsLogin,
    PremiumRequired,
    Error,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::NotConfigured => "not_configured",
            Status::Connecting => "connecting",
            Status::Connected => "connected",
            Status::NeedsLogin => "needs_login",
            Status::PremiumRequired => "premium_required",
            Status::Error => "error",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub active: bool,
    pub volume: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    /// Spotify URI, or a stand-in for ads and unknown items.
    pub key: String,
    pub kind: String,
    pub title: String,
    pub artists: String,
    pub album: String,
    pub art_url: Option<String>,
    pub duration_ms: u64,
}

/// Actions Spotify currently allows (from `actions.disallows`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Allowed {
    pub play_pause: bool,
    pub next: bool,
    pub prev: bool,
    pub seek: bool,
    pub shuffle: bool,
    pub repeat: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Player {
    pub item: Option<Item>,
    pub playing: bool,
    pub progress_ms: u64,
    pub updated_at: u64,
    pub shuffle: bool,
    /// Smart Shuffle keeps `shuffle_state` true, so it is tracked on its own to refresh Up next.
    pub smart_shuffle: bool,
    pub repeat: String,
    pub device: Option<Device>,
    pub allowed: Allowed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub time_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Lyrics {
    #[default]
    None,
    Loading,
    Instrumental,
    Ready(Arc<Vec<Line>>),
}

pub struct Art {
    pub rev: u64,
    pub mime: &'static str,
    pub bytes: Arc<Vec<u8>>,
}

/// One upcoming track from /me/player/queue, with its small cover once downloaded.
#[derive(Clone, Debug, PartialEq)]
pub struct Upcoming {
    pub title: String,
    pub artists: String,
    pub art_url: Option<String>,
}

/// Upcoming tracks shown by the widget.
pub const QUEUE_LEN: usize = 5;

#[derive(Default)]
pub struct State {
    pub version: u64,
    pub status: Status,
    pub message: Option<String>,
    pub client_id: Option<String>,
    pub account: Option<(String, String)>,
    pub scopes: Vec<String>,
    pub player: Option<Player>,
    pub devices: Vec<Device>,
    pub rev: u64,
    pub art: Option<Art>,
    pub art_pending: bool,
    pub lyrics: Lyrics,
    pub queue: Vec<Upcoming>,
    /// Bumped whenever `queue` changes; cover URLs carry it so a widget never shows a stale cover.
    pub queue_rev: u64,
    pub queue_art: Vec<Option<(&'static str, Arc<Vec<u8>>)>>,
    pub last_seen: u64,
    pub rate_limited_until: Option<u64>,
    pub pending: Option<auth::Pending>,
    pub tokens: Option<auth::Tokens>,
    /// Set by commands and sign-in so the poller runs at once.
    pub poke: bool,
    published_at: u64,
}

pub struct Hub {
    pub state: Mutex<State>,
    pub changed: Condvar,
}

pub static HUB: LazyLock<Hub> = LazyLock::new(|| Hub { state: Mutex::new(State::default()), changed: Condvar::new() });

pub fn lock() -> MutexGuard<'static, State> {
    HUB.state.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn publish(st: &mut State) {
    st.version += 1;
    st.published_at = now_ms();
    HUB.changed.notify_all();
}

/// Publishes on change, or as a heartbeat so widgets can tell fresh data from stale data.
pub fn publish_if(st: &mut State, changed: bool) {
    if changed || now_ms() >= st.published_at + HEARTBEAT_MS {
        publish(st);
    }
}

/// Loads a previous sign-in (client id + encrypted refresh token) at start.
pub fn init() {
    let mut st = lock();
    match auth::load() {
        Some((client_id, Some(refresh))) => {
            st.client_id = Some(client_id);
            st.tokens = Some(auth::Tokens { access: String::new(), expires_at: 0, refresh });
            st.status = Status::Connecting;
        }
        Some((client_id, None)) => {
            st.client_id = Some(client_id);
            st.status = Status::NeedsLogin;
        }
        None => st.status = Status::NotConfigured,
    }
    publish(&mut st);
}

pub fn disconnect() {
    auth::disconnect();
}

fn device_json(d: &Device) -> Value {
    json!({ "id": d.id, "name": d.name, "type": d.kind, "active": d.active, "volume": d.volume })
}

/// What the widget receives. Tokens, client id and account details are never included.
pub fn snapshot(st: &State) -> Value {
    let p = st.player.as_ref();
    let item = p.and_then(|p| p.item.as_ref()).map(|it| {
        json!({
            "id": it.key,
            "rev": st.rev,
            "type": it.kind,
            "title": it.title,
            "artists": it.artists,
            "album": it.album,
            "art": st.art.as_ref().filter(|a| a.rev == st.rev).map(|a| json!({ "url": format!("/api/spotify/art?r={}", a.rev) })),
            "artState": if st.art.as_ref().is_some_and(|a| a.rev == st.rev) { "ready" } else if st.art_pending { "pending" } else { "none" },
            "duration": it.duration_ms as f64 / 1000.0,
        })
    });
    let allowed = p.map(|p| p.allowed).unwrap_or_default();
    let device = p.and_then(|p| p.device.as_ref());
    let lyrics = match &st.lyrics {
        Lyrics::None => json!({ "state": "none" }),
        Lyrics::Loading => json!({ "state": "loading" }),
        Lyrics::Instrumental => json!({ "state": "instrumental" }),
        Lyrics::Ready(lines) => json!({ "state": "ready", "lines": lines.iter().map(|l| json!({ "timeMs": l.time_ms, "text": l.text })).collect::<Vec<_>>() }),
    };
    json!({
        "schema": "spotify/1",
        "source": { "kind": "companion", "connected": true, "lastSeen": st.last_seen },
        "account": { "status": st.status.word(), "message": st.message },
        "device": device.map(device_json),
        "devices": st.devices.iter().map(device_json).collect::<Vec<_>>(),
        "item": item,
        "playback": p.map(|p| json!({
            "playing": p.playing, "position": p.progress_ms as f64 / 1000.0, "updatedAt": p.updated_at,
            "shuffle": p.shuffle, "repeat": p.repeat,
        })),
        "actions": {
            "playPause": allowed.play_pause, "next": allowed.next, "prev": allowed.prev, "seek": allowed.seek,
            "shuffle": allowed.shuffle, "repeat": allowed.repeat,
            "volume": device.is_some_and(|d| d.volume.is_some()),
        },
        "rateLimitedUntil": st.rate_limited_until.filter(|&t| t > now_ms()),
        "lyrics": lyrics,
        "queueRev": st.queue_rev,
        "queue": st.queue.iter().enumerate().map(|(i, q)| json!({
            "title": q.title,
            "artist": q.artists,
            "art": st.queue_art.get(i).and_then(Option::as_ref).map(|_| json!({ "url": format!("/api/spotify/queue-art?i={i}&q={}", st.queue_rev) })),
        })).collect::<Vec<_>>(),
    })
}

pub fn queue_art_for(st: &State, index: usize, queue_rev: u64) -> Option<(&'static str, Arc<Vec<u8>>)> {
    (queue_rev == st.queue_rev).then(|| st.queue_art.get(index)?.as_ref().map(|(m, b)| (*m, Arc::clone(b)))).flatten()
}

/// What the companion window shows. Only the last 4 characters of the client id leave the store.
pub fn page(st: &State) -> Value {
    let p = st.player.as_ref();
    let it = p.and_then(|p| p.item.as_ref());
    json!({
        "status": st.status.word(),
        "message": st.message,
        "pending": st.pending.is_some(),
        "clientIdTail": st.client_id.as_deref().map(|c| c[c.len().saturating_sub(4)..].to_string()),
        "redirectUri": auth::REDIRECT_URI,
        "account": st.account.as_ref().map(|(name, product)| json!({ "name": name, "product": product })),
        "scopes": st.scopes,
        "device": p.and_then(|p| p.device.as_ref()).map(device_json),
        "item": it.map(|i| json!({ "title": i.title, "artists": i.artists, "playing": p.is_some_and(|p| p.playing) })),
        "lastSeen": st.last_seen,
        "rateLimitedUntil": st.rate_limited_until.filter(|&t| t > now_ms()),
        "lyrics": match &st.lyrics { Lyrics::Ready(_) => "ready", Lyrics::Loading => "loading", Lyrics::Instrumental => "instrumental", Lyrics::None => "none" },
    })
}

pub fn art_for(st: &State, rev: u64) -> Option<(&'static str, Arc<Vec<u8>>)> {
    st.art.as_ref().filter(|a| a.rev == rev && rev == st.rev).map(|a| (a.mime, Arc::clone(&a.bytes)))
}

/// One Web API call derived from a widget command.
#[derive(Debug, PartialEq)]
pub struct Call {
    pub method: &'static str,
    pub path: String,
    pub body: Option<String>,
    /// How many times to send it: `skipTo` is that many "next" in a row (Spotify has no jump-to-queue-item).
    pub times: u8,
}

/// Validates a widget command against the state. Errors are (HTTP status, code).
pub fn resolve(st: &State, body: &Value) -> Result<Call, (u16, &'static str)> {
    if st.status != Status::Connected {
        return Err((409, "not_connected"));
    }
    if st.rate_limited_until.is_some_and(|t| t > now_ms()) {
        return Err((429, "rate_limited"));
    }
    let cmd = body["cmd"].as_str().ok_or((400, "bad_request"))?;
    let p = st.player.as_ref();
    let a = p.map(|p| p.allowed).unwrap_or_default();
    // Skips and seeks aimed at a track that already changed are refused, never replayed.
    let current = |need: bool| -> Result<(), (u16, &'static str)> {
        if !need {
            return Err((422, "unsupported"));
        }
        if body["rev"].as_u64() != Some(st.rev) {
            return Err((409, "track_changed"));
        }
        Ok(())
    };
    let call = |method: &'static str, path: String| Call { method, path, body: None, times: 1 };
    Ok(match cmd {
        "playPause" => {
            if !a.play_pause {
                return Err((422, "unsupported"));
            }
            if p.is_some_and(|p| p.playing) { call("PUT", "/me/player/pause".into()) } else { call("PUT", "/me/player/play".into()) }
        }
        "next" => {
            current(a.next)?;
            call("POST", "/me/player/next".into())
        }
        // Tap on an up-next row: skip forward to it. The list the widget saw must still be current.
        "skipTo" => {
            current(a.next)?;
            if body["q"].as_u64() != Some(st.queue_rev) {
                return Err((409, "queue_changed"));
            }
            let i = body["value"].as_u64().filter(|&i| (i as usize) < st.queue.len()).ok_or((400, "bad_request"))?;
            Call { times: i as u8 + 1, ..call("POST", "/me/player/next".into()) }
        }
        "prev" => {
            current(a.prev)?;
            call("POST", "/me/player/previous".into())
        }
        "seek" => {
            current(a.seek)?;
            let v = body["value"].as_f64().filter(|v| v.is_finite() && *v >= 0.0).ok_or((400, "bad_request"))?;
            let max = p.and_then(|p| p.item.as_ref()).map_or(0, |i| i.duration_ms);
            call("PUT", format!("/me/player/seek?position_ms={}", ((v * 1000.0) as u64).min(max)))
        }
        "shuffle" => {
            if !a.shuffle {
                return Err((422, "unsupported"));
            }
            let on = body["value"].as_bool().ok_or((400, "bad_request"))?;
            call("PUT", format!("/me/player/shuffle?state={on}"))
        }
        "repeat" => {
            if !a.repeat {
                return Err((422, "unsupported"));
            }
            let mode = body["value"].as_str().filter(|m| ["off", "context", "track"].contains(m)).ok_or((400, "bad_request"))?;
            call("PUT", format!("/me/player/repeat?state={mode}"))
        }
        "volume" => {
            if !p.and_then(|p| p.device.as_ref()).is_some_and(|d| d.volume.is_some()) {
                return Err((422, "unsupported"));
            }
            let v = body["value"].as_f64().filter(|v| v.is_finite()).ok_or((400, "bad_request"))?;
            call("PUT", format!("/me/player/volume?volume_percent={}", v.round().clamp(0.0, 100.0) as u8))
        }
        "transfer" => {
            let id = body["deviceId"].as_str().ok_or((400, "bad_request"))?;
            // Only a device Spotify listed for this account; the id never reaches a URL.
            let d = st.devices.iter().find(|d| d.id == id).ok_or((404, "device_gone"))?;
            Call { method: "PUT", path: "/me/player".into(), body: Some(json!({ "device_ids": [d.id], "play": true }).to_string()), times: 1 }
        }
        _ => return Err((400, "unknown_command")),
    })
}

/// Parses LRC text (`[mm:ss.xx] line`) into timed lines.
pub fn parse_lrc(raw: &str) -> Vec<Line> {
    let mut lines: Vec<Line> = raw
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix('[')?;
            let (stamp, text) = rest.split_once(']')?;
            let (m, s) = stamp.split_once(':')?;
            let ms = m.trim().parse::<u64>().ok()?.checked_mul(60_000)?.checked_add((s.trim().parse::<f64>().ok()? * 1000.0) as u64)?;
            Some(Line { time_ms: ms, text: text.trim().to_string() })
        })
        .collect();
    lines.sort_by_key(|l| l.time_ms);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected() -> State {
        State {
            status: Status::Connected,
            rev: 7,
            player: Some(Player {
                item: Some(Item { key: "spotify:track:1".into(), kind: "track".into(), duration_ms: 200_000, ..Default::default() }),
                playing: true,
                repeat: "off".into(),
                device: Some(Device { id: "d1".into(), volume: Some(40), active: true, ..Default::default() }),
                allowed: Allowed { play_pause: true, next: true, prev: true, seek: true, shuffle: true, repeat: false },
                ..Default::default()
            }),
            devices: vec![Device { id: "d1".into(), ..Default::default() }],
            ..Default::default()
        }
    }

    #[test]
    fn commands_are_an_allowlist() {
        let st = connected();
        let r = |v: Value| resolve(&st, &v);
        assert_eq!(r(json!({ "cmd": "playPause" })).unwrap().path, "/me/player/pause");
        assert_eq!(r(json!({ "cmd": "next", "rev": 6 })).unwrap_err(), (409, "track_changed"));
        assert_eq!(r(json!({ "cmd": "next", "rev": 7 })).unwrap().method, "POST");
        assert_eq!(r(json!({ "cmd": "seek", "rev": 7, "value": 999 })).unwrap().path, "/me/player/seek?position_ms=200000");
        assert_eq!(r(json!({ "cmd": "repeat", "value": "track" })).unwrap_err(), (422, "unsupported"));
        assert_eq!(r(json!({ "cmd": "shuffle", "value": "yes" })).unwrap_err(), (400, "bad_request"));
        assert_eq!(r(json!({ "cmd": "volume", "value": 140 })).unwrap().path, "/me/player/volume?volume_percent=100");
        assert_eq!(r(json!({ "cmd": "transfer", "deviceId": "../../me" })).unwrap_err(), (404, "device_gone"));
        assert_eq!(r(json!({ "cmd": "transfer", "deviceId": "d1" })).unwrap().body.unwrap(), r#"{"device_ids":["d1"],"play":true}"#);
        assert_eq!(r(json!({ "cmd": "delete" })).unwrap_err(), (400, "unknown_command"));
        let mut q = connected();
        q.queue = vec![Upcoming { title: "A".into(), artists: "X".into(), art_url: None }, Upcoming { title: "B".into(), artists: "Y".into(), art_url: None }];
        q.queue_rev = 4;
        let skip = |v: Value| resolve(&q, &v);
        assert_eq!(skip(json!({ "cmd": "skipTo", "rev": 7, "q": 4, "value": 1 })).unwrap().times, 2);
        assert_eq!(skip(json!({ "cmd": "skipTo", "rev": 7, "q": 3, "value": 1 })).unwrap_err(), (409, "queue_changed"));
        assert_eq!(skip(json!({ "cmd": "skipTo", "rev": 7, "q": 4, "value": 2 })).unwrap_err(), (400, "bad_request"));
        assert_eq!(skip(json!({ "cmd": "skipTo", "rev": 6, "q": 4, "value": 0 })).unwrap_err(), (409, "track_changed"));
        let mut off = connected();
        off.status = Status::NeedsLogin;
        assert_eq!(resolve(&off, &json!({ "cmd": "playPause" })).unwrap_err(), (409, "not_connected"));
    }

    #[test]
    fn snapshot_never_carries_secrets_and_art_follows_rev() {
        let mut st = connected();
        st.client_id = Some("0f3c9a1d7e5b4c2a8d6e0b1f9a7c3f2a".into());
        st.tokens = Some(auth::Tokens { access: "secret-access".into(), refresh: "secret-refresh".into(), expires_at: 0 });
        st.art = Some(Art { rev: 6, mime: "image/jpeg", bytes: Arc::new(vec![1]) });
        let text = snapshot(&st).to_string();
        assert!(!text.contains("secret") && !text.contains("0f3c9a1d"));
        assert_eq!(snapshot(&st)["item"]["art"], Value::Null, "art of an older revision is not offered");
        assert!(art_for(&st, 6).is_none());
        assert_eq!(page(&st)["clientIdTail"], "3f2a");
    }

    #[test]
    fn lrc_parsing() {
        let lines = parse_lrc("[00:12.50] Second\n[00:01.00]First\nno stamp\n[01:02.3] Third\n[00:05.00]");
        assert_eq!(lines.iter().map(|l| l.time_ms).collect::<Vec<_>>(), vec![1000, 5000, 12500, 62300]);
        assert_eq!(lines[0].text, "First");
        assert_eq!(lines[1].text, "");
        // A huge minute count from LRCLIB is dropped, not wrapped or panicking.
        assert!(parse_lrc("[99999999999999999:00.00] x").is_empty());
    }
}
