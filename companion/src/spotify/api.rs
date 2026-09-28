//! Spotify Web API polling, artwork, LRCLIB lyrics and command execution.
//! Polls /me/player every 1 s while playing, 3 s when paused, 8 s with nothing playing,
//! and waits on Retry-After when Spotify rate-limits.
use super::{auth, lock, parse_lrc, publish, publish_if, Allowed, Art, Call, Device, Item, Line, Lyrics, Player, Status, Upcoming, HUB, QUEUE_LEN};
use crate::util::{now_ms, read_limited, SEC};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

const API: &str = "https://api.spotify.com/v1";
const LRCLIB: &str = "https://lrclib.net/api";
const USER_AGENT: &str = "icue-edge-companion (XENEON EDGE widget)";
const MAX_ART_BYTES: u64 = 4 * 1024 * 1024;
const MAX_LYRICS_BYTES: u64 = 512 * 1024;
const MAX_API_BYTES: usize = 2 * 1024 * 1024;
/// Reported and predicted positions may drift this much before a snapshot is re-sent.
const DRIFT_MS: u64 = 1500;

enum ApiError {
    Unauthorized,
    Premium,
    Forbidden,
    RateLimited(u64),
    Other(String),
}

fn direct_get(url: &str) -> Result<ureq::Response, ureq::Error> {
    ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(8)).build().get(url).set("User-Agent", USER_AGENT).call()
}

fn request(method: &str, path: &str, body: Option<&str>) -> Result<Option<Value>, ApiError> {
    let token = auth::access_token().map_err(|s| match s {
        Status::NeedsLogin => ApiError::Unauthorized,
        Status::NotConfigured => ApiError::Other("Spotify is not connected".into()),
        _ => ApiError::Other("Spotify is unreachable".into()),
    })?;
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(8)).build();
    let req = agent.request(method, &format!("{API}{path}"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("User-Agent", "icue-edge-companion");
    let res = match body {
        Some(b) => req.set("Content-Type", "application/json").send_string(b),
        None if method == "GET" => req.call(),
        None => req.send_bytes(&[]),
    };
    match res {
        Ok(r) if r.status() == 204 => Ok(None),
        Ok(r) => {
            let bytes = read_limited(r.into_reader(), MAX_API_BYTES).map_err(|e| ApiError::Other(e.to_string()))?;
            Ok(if bytes.iter().all(u8::is_ascii_whitespace) { None } else { serde_json::from_slice(&bytes).ok() })
        }
        Err(ureq::Error::Status(401, _)) => Err(ApiError::Unauthorized),
        Err(ureq::Error::Status(403, r)) => {
            let v: Value = read_limited(r.into_reader(), MAX_API_BYTES).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
            let reason = v["error"]["reason"].as_str().unwrap_or("");
            Err(if reason == "PREMIUM_REQUIRED" { ApiError::Premium } else { ApiError::Forbidden })
        }
        Err(ureq::Error::Status(429, r)) => {
            let secs = r.header("retry-after").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(30);
            Err(ApiError::RateLimited(secs.clamp(1, 3600) * SEC))
        }
        Err(ureq::Error::Status(code, _)) => Err(ApiError::Other(format!("Spotify answered {code}"))),
        Err(_) => Err(ApiError::Other("Spotify is unreachable".into())),
    }
}

/// Calls once more after refreshing the token when Spotify answers 401.
fn call_api(method: &str, path: &str, body: Option<&str>) -> Result<Option<Value>, ApiError> {
    match request(method, path, body) {
        Err(ApiError::Unauthorized) => {
            auth::expire_access();
            request(method, path, body)
        }
        other => other,
    }
}

fn apply_error(e: &ApiError) {
    let mut st = lock();
    match e {
        ApiError::Unauthorized => {
            st.status = Status::NeedsLogin;
            st.message = Some("Spotify ended the session. Connect again.".into());
        }
        ApiError::Premium => {
            st.status = Status::PremiumRequired;
            st.message = Some("Spotify Premium is required for playback control.".into());
        }
        ApiError::Forbidden => st.message = Some("Spotify refused the request. Check User Management in your Spotify app.".into()),
        ApiError::RateLimited(ms) => st.rate_limited_until = Some(now_ms() + ms),
        ApiError::Other(m) => st.message = Some(m.clone()),
    }
    publish(&mut st);
}

// ---------- parsing ----------

fn device_from(v: &Value) -> Device {
    Device {
        id: v["id"].as_str().unwrap_or("").to_string(),
        name: v["name"].as_str().unwrap_or("Spotify device").to_string(),
        kind: v["type"].as_str().unwrap_or("Device").to_string(),
        active: v["is_active"].as_bool().unwrap_or(false),
        volume: if v["supports_volume"].as_bool() == Some(false) { None } else { v["volume_percent"].as_u64().map(|x| x.min(100) as u8) },
    }
}

fn item_from(player: &Value) -> Option<Item> {
    let kind = player["currently_playing_type"].as_str().unwrap_or("unknown").to_string();
    let it = &player["item"];
    if kind == "ad" {
        return Some(Item { key: "ad".into(), kind, title: "Advertisement".into(), artists: "Spotify".into(), ..Default::default() });
    }
    if it.is_null() {
        return None;
    }
    let names = |a: &Value| a.as_array().map(|x| x.iter().filter_map(|n| n["name"].as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let (artists, album, images) = if kind == "episode" {
        (it["show"]["name"].as_str().unwrap_or("").to_string(), it["show"]["publisher"].as_str().unwrap_or("").to_string(), &it["images"])
    } else {
        (names(&it["artists"]), it["album"]["name"].as_str().unwrap_or("").to_string(), &it["album"]["images"])
    };
    Some(Item {
        key: it["uri"].as_str().or(it["id"].as_str()).unwrap_or("unknown").to_string(),
        kind,
        title: it["name"].as_str().unwrap_or("").to_string(),
        artists,
        album,
        art_url: images.as_array().and_then(|a| a.first()).and_then(|i| i["url"].as_str()).map(String::from),
        duration_ms: it["duration_ms"].as_u64().unwrap_or(0),
    })
}

fn player_from(v: &Value, now: u64) -> Player {
    let d = &v["actions"]["disallows"];
    let no = |k: &str| d[k].as_bool().unwrap_or(false);
    let playing = v["is_playing"].as_bool().unwrap_or(false);
    Player {
        item: item_from(v),
        playing,
        progress_ms: v["progress_ms"].as_u64().unwrap_or(0),
        updated_at: now,
        shuffle: v["shuffle_state"].as_bool().unwrap_or(false),
        repeat: v["repeat_state"].as_str().unwrap_or("off").to_string(),
        device: v.get("device").filter(|d| d.is_object()).map(device_from),
        allowed: Allowed {
            play_pause: !(if playing { no("pausing") } else { no("resuming") }),
            next: !no("skipping_next"),
            prev: !no("skipping_prev"),
            seek: !no("seeking"),
            shuffle: !no("toggling_shuffle"),
            repeat: !(no("toggling_repeat_context") && no("toggling_repeat_track")),
        },
    }
}

/// Whether the new reading changes anything a widget shows (position only past DRIFT_MS).
fn differs(old: &Player, new: &Player) -> bool {
    let predicted = old.progress_ms + if old.playing { new.updated_at.saturating_sub(old.updated_at) } else { 0 };
    let drift = predicted.abs_diff(new.progress_ms);
    old.item != new.item || old.playing != new.playing || old.shuffle != new.shuffle || old.repeat != new.repeat
        || old.device != new.device || old.allowed != new.allowed || drift > DRIFT_MS
}

// ---------- artwork and lyrics (background, dropped if the track moved on) ----------

fn art_host_ok(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.contains(['@', ':']) && (host.ends_with(".scdn.co") || host.ends_with(".spotifycdn.com"))
}

fn fetch_art(rev: u64, url: String) {
    let bytes = (|| {
        if !art_host_ok(&url) {
            return None;
        }
        let r = direct_get(&url).ok()?;
        let mut buf = Vec::new();
        r.into_reader().take(MAX_ART_BYTES + 1).read_to_end(&mut buf).ok()?;
        (buf.len() as u64 <= MAX_ART_BYTES).then_some(buf)
    })();
    let mut st = lock();
    if st.rev != rev {
        return;
    }
    st.art_pending = false;
    st.art = bytes.and_then(|b| crate::media::sniff(&b).map(|mime| Art { rev, mime, bytes: Arc::new(b) }));
    publish(&mut st);
}

static LYRICS_CACHE: LazyLock<Mutex<HashMap<String, Lyrics>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn lrclib(path: &str, params: &[(&str, String)]) -> Option<Value> {
    let query = params.iter().map(|(k, v)| format!("{k}={}", auth::percent(v))).collect::<Vec<_>>().join("&");
    let r = direct_get(&format!("{LRCLIB}/{path}?{query}")).ok()?;
    let bytes = read_limited(r.into_reader(), MAX_LYRICS_BYTES as usize).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn lyrics_from(v: &Value) -> Option<Lyrics> {
    if v["instrumental"].as_bool() == Some(true) {
        return Some(Lyrics::Instrumental);
    }
    let lines: Vec<Line> = parse_lrc(v["syncedLyrics"].as_str()?);
    (!lines.is_empty()).then(|| Lyrics::Ready(Arc::new(lines)))
}

/// LRCLIB exact match first (title, artist, album, duration), then the closest synced search result.
/// None when LRCLIB could not be reached: that answer is not cached, the next play asks again.
fn find_lyrics(item: &Item) -> Option<Lyrics> {
    let artist = item.artists.split(", ").next().unwrap_or("").to_string();
    let secs = item.duration_ms / 1000;
    let exact = lrclib("get", &[("track_name", item.title.clone()), ("artist_name", artist.clone()), ("album_name", item.album.clone()), ("duration", secs.to_string())]);
    if let Some(l) = exact.as_ref().and_then(lyrics_from) {
        return Some(l);
    }
    let found = lrclib("search", &[("track_name", item.title.clone()), ("artist_name", artist)])?;
    let best = found
        .as_array()
        .and_then(|a| {
            a.iter()
                .filter(|r| r["syncedLyrics"].is_string())
                .min_by_key(|r| (r["duration"].as_f64().unwrap_or(0.0) as i64 - secs as i64).abs())
                .filter(|r| (r["duration"].as_f64().unwrap_or(0.0) as i64 - secs as i64).abs() <= 5)
                .and_then(lyrics_from)
        })
        .unwrap_or(Lyrics::None);
    Some(best)
}

fn fetch_lyrics(rev: u64, item: Item) {
    let cached = LYRICS_CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(&item.key).cloned();
    let lyrics = cached.unwrap_or_else(|| {
        let Some(l) = find_lyrics(&item) else { return Lyrics::None };
        let mut cache = LYRICS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        // ponytail: whole cache cleared at 64 tracks; an LRU if re-fetches ever matter.
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(item.key.clone(), l.clone());
        l
    });
    let mut st = lock();
    if st.rev == rev {
        st.lyrics = lyrics;
        publish(&mut st);
    }
}

// ---------- queue (up next) ----------

/// The next tracks from a /me/player/queue answer, with the ~300 px cover (enough for a row thumbnail).
fn upcoming_from(v: &Value) -> Vec<Upcoming> {
    let names = |a: &Value| a.as_array().map(|x| x.iter().filter_map(|n| n["name"].as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let list = v["queue"].as_array().map(Vec::as_slice).unwrap_or_default();
    list.iter()
        .take(QUEUE_LEN)
        .map(|it| {
            let episode = it["type"].as_str() == Some("episode");
            let images = if episode { &it["images"] } else { &it["album"]["images"] };
            // Spotify lists images largest first: take the smallest one still at least 160 px wide.
            let imgs = images.as_array().map(Vec::as_slice).unwrap_or_default();
            let art = imgs.iter().rev().find(|i| i["width"].as_u64().unwrap_or(0) >= 160).or(imgs.first());
            Upcoming {
                title: it["name"].as_str().unwrap_or("").to_string(),
                artists: if episode { it["show"]["name"].as_str().unwrap_or("").to_string() } else { names(&it["artists"]) },
                art_url: art.and_then(|i| i["url"].as_str()).map(String::from),
            }
        })
        .collect()
}

static QUEUE_GEN: AtomicU64 = AtomicU64::new(0);
static QUEUE_AT: AtomicU64 = AtomicU64::new(0);
const QUEUE_EVERY_MS: u64 = 30 * SEC;
const MAX_THUMB_BYTES: u64 = 1024 * 1024;

fn fetch_queue() {
    let gen = QUEUE_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let Ok(v) = call_api("GET", "/me/player/queue", None) else { return };
    let queue = v.as_ref().map(upcoming_from).unwrap_or_default();
    // Covers already downloaded for the same URL are kept; only new ones are fetched.
    let known: Vec<(Option<String>, Option<(&'static str, Arc<Vec<u8>>)>)> = {
        let st = lock();
        st.queue.iter().map(|q| q.art_url.clone()).zip(st.queue_art.iter().cloned()).collect()
    };
    let art = queue
        .iter()
        .map(|q| {
            let url = q.art_url.as_ref()?;
            if let Some((_, a)) = known.iter().find(|(u, a)| u.as_ref() == Some(url) && a.is_some()) {
                return a.clone();
            }
            if !art_host_ok(url) {
                return None;
            }
            let mut buf = Vec::new();
            direct_get(url).ok()?.into_reader().take(MAX_THUMB_BYTES + 1).read_to_end(&mut buf).ok()?;
            (buf.len() as u64 <= MAX_THUMB_BYTES).then_some(())?;
            crate::media::sniff(&buf).map(|mime| (mime, Arc::new(buf)))
        })
        .collect::<Vec<_>>();
    let mut st = lock();
    // A newer fetch started meanwhile: its answer wins.
    if QUEUE_GEN.load(Ordering::SeqCst) != gen {
        return;
    }
    let changed = st.queue != queue || st.queue_art.iter().map(Option::is_some).ne(art.iter().map(Option::is_some));
    if changed {
        st.queue = queue;
        st.queue_art = art;
        st.queue_rev += 1;
        publish(&mut st);
    }
}

// ---------- polling ----------

fn fetch_profile() {
    if let Ok(Some(v)) = call_api("GET", "/me", None) {
        let name = v["display_name"].as_str().or(v["id"].as_str()).unwrap_or("Spotify user").to_string();
        // Since February 2026 Spotify omits `product` for new development-mode apps:
        // a missing value proves nothing, only a 403 PREMIUM_REQUIRED does.
        let product = v["product"].as_str().unwrap_or("").to_string();
        let mut st = lock();
        if !product.is_empty() && product != "premium" {
            st.status = Status::PremiumRequired;
            st.message = Some("Spotify Premium is required for playback control.".into());
        }
        st.account = Some((name, product));
        publish(&mut st);
    }
}

fn fetch_devices() {
    if let Ok(Some(v)) = call_api("GET", "/me/player/devices", None) {
        let devices: Vec<Device> = v["devices"].as_array().map(|a| a.iter().map(device_from).filter(|d| !d.id.is_empty()).collect()).unwrap_or_default();
        let mut st = lock();
        let changed = st.devices != devices;
        st.devices = devices;
        publish_if(&mut st, changed);
    }
}

/// One reading of /me/player. Returns the delay before the next one.
fn poll_once(last_devices: &mut u64) -> Duration {
    let now = now_ms();
    let reading = call_api("GET", "/me/player?additional_types=episode", None);
    match reading {
        Err(e) => {
            let wait = if let ApiError::RateLimited(ms) = e { Duration::from_millis(ms) } else { Duration::from_secs(10) };
            apply_error(&e);
            wait
        }
        Ok(v) => {
            let player = v.as_ref().map(|v| player_from(v, now));
            let (spawn, wait) = {
                let mut st = lock();
                if st.status == Status::Connecting {
                    st.status = Status::Connected;
                }
                if st.status == Status::Connected {
                    st.message = None;
                }
                st.rate_limited_until = None;
                st.last_seen = now;
                let old_key = st.player.as_ref().and_then(|p| p.item.as_ref()).map(|i| i.key.clone());
                let new_key = player.as_ref().and_then(|p| p.item.as_ref()).map(|i| i.key.clone());
                let mut spawn = None;
                if old_key != new_key {
                    st.rev += 1;
                    st.art = None;
                    let item = player.as_ref().and_then(|p| p.item.clone());
                    st.art_pending = item.as_ref().is_some_and(|i| i.art_url.is_some());
                    st.lyrics = if item.as_ref().is_some_and(|i| i.kind == "track") { Lyrics::Loading } else { Lyrics::None };
                    spawn = item.map(|i| (st.rev, i));
                }
                let changed = match (&st.player, &player) {
                    (Some(a), Some(b)) => differs(a, b),
                    (None, None) => false,
                    _ => true,
                };
                if changed {
                    st.player = player.clone();
                }
                publish_if(&mut st, changed || spawn.is_some());
                let wait = match &player {
                    Some(p) if p.playing => 1,
                    Some(_) => 3,
                    None => 8,
                };
                (spawn, Duration::from_secs(wait))
            };
            // Up next: at every track change, and every 30 s while playing (the user may edit the queue).
            let playing = player.as_ref().is_some_and(|p| p.playing);
            if spawn.is_some() || (playing && now >= QUEUE_AT.load(Ordering::Relaxed) + QUEUE_EVERY_MS) {
                QUEUE_AT.store(now, Ordering::Relaxed);
                std::thread::spawn(fetch_queue);
            }
            if let Some((rev, item)) = spawn {
                if let Some(url) = item.art_url.clone() {
                    std::thread::spawn(move || fetch_art(rev, url));
                }
                if item.kind == "track" {
                    std::thread::spawn(move || fetch_lyrics(rev, item));
                }
            }
            // The device list is only needed to offer "Play on …" when nothing is active.
            if player.is_none() && now >= *last_devices + 10 * SEC {
                *last_devices = now;
                fetch_devices();
            }
            wait
        }
    }
}

pub fn run() {
    super::init();
    let mut last_devices = 0u64;
    let mut profiled_for: Option<String> = None;
    loop {
        let status = lock().status;
        // Reading playback works without Premium, so keep polling in that state too.
        let wait = if matches!(status, Status::Connected | Status::Connecting | Status::PremiumRequired) {
            let who = lock().client_id.clone();
            if profiled_for != who {
                profiled_for = who;
                fetch_profile();
            }
            poll_once(&mut last_devices)
        } else {
            Duration::from_secs(5)
        };
        // Commands and sign-in set `poke` to cut the wait short.
        let st = lock();
        let (mut st, _) = HUB.changed.wait_timeout_while(st, wait, |s| !s.poke).unwrap_or_else(|e| e.into_inner());
        st.poke = false;
    }
}

/// Runs a validated widget command, then asks the poller for a fresh reading.
pub fn execute(call: &Call) -> Result<(), (u16, &'static str)> {
    let mut result = Ok(None);
    for n in 0..call.times.max(1) {
        if n > 0 {
            // Spotify applies skips in order only when they are not fired at the same instant.
            std::thread::sleep(Duration::from_millis(150));
        }
        result = call_api(call.method, &call.path, call.body.as_deref());
        if result.is_err() {
            break;
        }
    }
    let mut st = lock();
    st.poke = true;
    HUB.changed.notify_all();
    drop(st);
    match result {
        Ok(_) => Ok(()),
        Err(ApiError::RateLimited(ms)) => {
            apply_error(&ApiError::RateLimited(ms));
            Err((429, "rate_limited"))
        }
        Err(ApiError::Premium) => Err((403, "premium_required")),
        Err(ApiError::Forbidden) => Err((403, "refused_by_spotify")),
        Err(ApiError::Unauthorized) => Err((401, "needs_login")),
        Err(ApiError::Other(_)) => Err((502, "spotify_error")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn player_parsing_and_disallows() {
        let v = json!({
            "is_playing": true, "progress_ms": 5000, "shuffle_state": true, "repeat_state": "context",
            "currently_playing_type": "track",
            "device": { "id": "d1", "name": "Desktop", "type": "Computer", "is_active": true, "volume_percent": 65, "supports_volume": true },
            "actions": { "disallows": { "skipping_prev": true, "toggling_shuffle": true } },
            "item": { "uri": "spotify:track:x", "name": "Song", "duration_ms": 210000,
                      "artists": [{ "name": "A" }, { "name": "B" }],
                      "album": { "name": "Album", "images": [{ "url": "https://i.scdn.co/image/abc" }] } }
        });
        let p = player_from(&v, 1000);
        let it = p.item.as_ref().unwrap();
        assert_eq!((it.title.as_str(), it.artists.as_str(), it.album.as_str()), ("Song", "A, B", "Album"));
        assert_eq!(it.art_url.as_deref(), Some("https://i.scdn.co/image/abc"));
        assert!(p.allowed.next && !p.allowed.prev && !p.allowed.shuffle && p.allowed.repeat && p.allowed.play_pause);
        assert_eq!(p.device.as_ref().unwrap().volume, Some(65));
        let ad = player_from(&json!({ "currently_playing_type": "ad", "is_playing": true, "item": null }), 0);
        assert_eq!(ad.item.unwrap().title, "Advertisement");
        let fixed = device_from(&json!({ "id": "p", "supports_volume": false, "volume_percent": 30 }));
        assert_eq!(fixed.volume, None);
    }

    #[test]
    fn drift_threshold() {
        let a = Player { playing: true, progress_ms: 10_000, updated_at: 0, ..Default::default() };
        let near = Player { progress_ms: 11_200, updated_at: 1000, ..a.clone() };
        let jump = Player { progress_ms: 40_000, updated_at: 1000, ..a.clone() };
        assert!(!differs(&a, &near));
        assert!(differs(&a, &jump));
    }

    #[test]
    fn artwork_hosts() {
        assert!(art_host_ok("https://i.scdn.co/image/ab67616d0000b273"));
        assert!(art_host_ok("https://image-cdn-ak.spotifycdn.com/image/x"));
        assert!(!art_host_ok("http://i.scdn.co/image/x"));
        assert!(!art_host_ok("https://evil.example/i.scdn.co/x"));
        assert!(!art_host_ok("https://i.scdn.co.evil.example/x"));
        assert!(!art_host_ok("https://evil.example?.scdn.co/x"));
        assert!(!art_host_ok("https://user@i.scdn.co/x"));
    }

    #[test]
    fn lyrics_selection() {
        assert_eq!(lyrics_from(&json!({ "instrumental": true })), Some(Lyrics::Instrumental));
        assert_eq!(lyrics_from(&json!({ "syncedLyrics": null, "plainLyrics": "x" })), None);
        match lyrics_from(&json!({ "syncedLyrics": "[00:01.00] Hi" })) {
            Some(Lyrics::Ready(l)) => assert_eq!(l[0].text, "Hi"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn queue_parsing_keeps_five_with_mid_size_covers() {
        let track = |n: u32| json!({ "type": "track", "name": format!("T{n}"), "artists": [{ "name": "A" }, { "name": "B" }],
            "album": { "images": [{ "url": "https://i.scdn.co/640", "width": 640 }, { "url": "https://i.scdn.co/300", "width": 300 }, { "url": "https://i.scdn.co/64", "width": 64 }] } });
        let episode = json!({ "type": "episode", "name": "E", "show": { "name": "Show" }, "images": [{ "url": "https://i.scdn.co/e", "width": 640 }] });
        let q = upcoming_from(&json!({ "queue": [track(1), episode, track(3), track(4), track(5), track(6)] }));
        assert_eq!(q.len(), QUEUE_LEN);
        assert_eq!((q[0].title.as_str(), q[0].artists.as_str(), q[0].art_url.as_deref()), ("T1", "A, B", Some("https://i.scdn.co/300")));
        assert_eq!((q[1].artists.as_str(), q[1].art_url.as_deref()), ("Show", Some("https://i.scdn.co/e")));
        assert!(upcoming_from(&json!({})).is_empty());
    }

    #[test]
    fn external_fetches_do_not_follow_redirects() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/start", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        assert_eq!(direct_get(&url).unwrap().status(), 302);
        server.join().unwrap();
    }
}
