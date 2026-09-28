//! Claude collector.
//! Tokens: Claude Code JSONL, `type == "assistant"` lines with `message.usage`.
//! Verified on local files: one API message is written on several lines (147 lines, 68 ids),
//! so usage is keyed by `message.id|requestId` and only growth over what was already counted
//! is added. `input_tokens` excludes cache: cache read / cache creation are separate categories.
//! Quotas: internal `GET /api/oauth/usage` with the OAuth token Claude Code stores in
//! `~/.claude/.credentials.json` (adapted from claude-rpc, MIT, © Inerthel). The token stays
//! in this process: never logged, stored or sent to the page.
use crate::usage::store::{hour_of, lock, update, Quota, QuotaSet, Session, Shared, Tok, ACTIVE_MS};
use crate::tail::Tail;
use crate::util::{home_dir, now_ms, parse_iso_ms, pseudonym, read_limited, DAY, MIN, SEC};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const OAUTH_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_USAGE_BETA: &str = "oauth-2025-04-20";
const QUOTA_SRC: &str = "Anthropic · /api/oauth/usage (internal interface)";

fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".claude"))
}

fn client_label(entrypoint: &str) -> String {
    let e = entrypoint.to_ascii_lowercase();
    if e.contains("desktop") {
        "Desktop".into()
    } else if e.contains("vscode") || e.contains("jetbrains") || e.contains("ide") {
        "IDE".into()
    } else if e.starts_with("sdk") {
        "SDK".into()
    } else if e == "cli" || e.is_empty() {
        "CLI".into()
    } else {
        entrypoint.chars().take(24).collect()
    }
}

struct Usage {
    key: String,
    session: String,
    client: String,
    model: String,
    sidechain: bool,
    tok: Tok,
    ts: u64,
}

fn parse_line(line: &str) -> Option<Usage> {
    if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let msg = v.get("message")?;
    let usage = msg.get("usage")?;
    let model = msg.get("model").and_then(Value::as_str).unwrap_or("");
    if model == "<synthetic>" {
        return None;
    }
    let id = msg.get("id").and_then(Value::as_str)?;
    let request = v.get("requestId").and_then(Value::as_str).unwrap_or("");
    let n = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
    Some(Usage {
        key: format!("{id}|{request}"),
        session: v.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string(),
        client: client_label(v.get("entrypoint").and_then(Value::as_str).unwrap_or("")),
        model: model.to_string(),
        sidechain: v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false),
        tok: Tok {
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
            reasoning: usage.pointer("/output_tokens_details/thinking_tokens").and_then(Value::as_u64).unwrap_or(0),
        },
        ts: v.get("timestamp").and_then(Value::as_str).and_then(parse_iso_ms).unwrap_or(0),
    })
}

fn apply(shared: &Shared, path_key: &str, usages: Vec<Usage>, offset: u64) {
    update(shared, |st| {
        let now = now_ms();
        let salt = st.saved.salt.clone();
        let file = st.saved.claude.files.entry(path_key.to_string()).or_default();
        file.offset = offset;
        file.last_seen = now;
        for u in usages {
            let counted = st.saved.claude.counted.entry(u.key).or_insert((Tok::default(), now));
            let growth = u.tok.growth_over(&counted.0);
            counted.0 = counted.0.max(&u.tok);
            counted.1 = now;
            if !growth.is_zero() {
                st.saved.claude.hours.entry(hour_of(u.ts)).or_default().add(&growth);
            }
            if u.session.is_empty() {
                continue;
            }
            let sid = u.session.clone();
            let s = st.saved.claude.sessions.entry(sid.clone()).or_insert_with(|| Session { id: pseudonym("c", &salt, &sid), started: u.ts, ..Default::default() });
            s.tokens.add(&growth);
            s.last_event = s.last_event.max(u.ts);
            s.client = u.client;
            if !u.sidechain {
                if !s.model.is_empty() && s.model != u.model && u.ts >= s.context_at {
                    let text = format!("Model changed: {} → {} (session {})", s.model, u.model, s.id);
                    st.claude.event(u.ts, text);
                }
                let s = st.saved.claude.sessions.get_mut(&sid).unwrap();
                if u.ts >= s.context_at {
                    s.model = u.model;
                    // Estimate: what the last request carried as context. Capacity is not in the log.
                    s.context_used = Some(u.tok.input + u.tok.cache_read + u.tok.cache_write);
                    s.context_cap = None;
                    s.context_at = u.ts;
                }
            }
        }
    });
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Files touched in the last 48 h. Between full walks, only directories modified
/// in that window are entered (a new session file updates its directory's mtime).
fn discover(root: &Path, full: bool, now: u64) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0u8)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            let recent = modified_ms(&meta) + 2 * DAY > now;
            let path = e.path();
            if meta.is_dir() {
                if depth < 3 && (full || recent) {
                    stack.push((path, depth + 1));
                }
            } else if recent && path.extension().is_some_and(|x| x == "jsonl") {
                out.push(path);
            }
        }
    }
    out
}

pub fn run(shared: Shared) {
    let root = claude_dir().join("projects");
    let mut tails: HashMap<String, Tail> = HashMap::new();
    let (mut last_discover, mut last_full) = (0u64, 0u64);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = now_ms();
        let detected = root.is_dir();
        {
            let mut st = lock(&shared);
            if st.claude.detected != detected {
                st.claude.detected = detected;
                // Widgets and the window must see this at once, not at the next unrelated update.
            st.dirty = true;
            st.version += 1;
            shared.changed.notify_all();
            }
            if st.paused || !detected {
                continue;
            }
        }
        if now >= last_discover + 10 * SEC {
            let full = now >= last_full + 10 * MIN;
            for path in discover(&root, full, now) {
                let key = path.to_string_lossy().to_string();
                if !tails.contains_key(&key) {
                    let offset = lock(&shared).saved.claude.files.get(&key).map(|f| f.offset).unwrap_or(0);
                    tails.insert(key, Tail::new(path, offset));
                }
            }
            tails.retain(|_, t| std::fs::metadata(&t.path).map(|m| modified_ms(&m) + 2 * DAY > now).unwrap_or(false));
            last_discover = now;
            if full {
                last_full = now;
            }
        }
        let mut error = None;
        for (key, tail) in tails.iter_mut() {
            if !tail.due(now) {
                continue;
            }
            let before = tail.offset;
            // Catch up in 1 MiB chunks; bounded per tick so one huge file cannot starve the others.
            for _ in 0..32 {
                match tail.read_lines() {
                    Ok(lines) if !lines.is_empty() => {
                        let usages = lines.iter().filter_map(|l| parse_line(l)).collect();
                        drop(lines);
                        apply(&shared, key, usages, tail.offset);
                    }
                    Ok(_) => break,
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::PermissionDenied {
                            error = Some("Access denied to a Claude log".to_string());
                        }
                        break;
                    }
                }
            }
            tail.schedule(now, tail.offset != before);
        }
        let mut st = lock(&shared);
        if st.claude.read_error != error {
            st.claude.read_error = error;
            // Widgets and the window must see this at once, not at the next unrelated update.
            st.dirty = true;
            st.version += 1;
            shared.changed.notify_all();
        }
    }
}

// ---------- quotas ----------

enum FetchError {
    NoToken(&'static str),
    Denied,
    RateLimited(u64),
    Network,
    Parse,
}

fn read_oauth_access_token() -> Result<String, FetchError> {
    let raw = read_limited(std::fs::File::open(claude_dir().join(".credentials.json")).map_err(|_| FetchError::NoToken("Claude token not found"))?, 1024 * 1024)
        .map_err(|_| FetchError::NoToken("Credentials file unreadable"))?;
    let raw = std::str::from_utf8(&raw).map_err(|_| FetchError::NoToken("Credentials file unreadable"))?;
    let value: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).map_err(|_| FetchError::NoToken("Credentials file unreadable"))?;
    let oauth = value.get("claudeAiOauth").ok_or(FetchError::NoToken("No Claude Code sign-in"))?;
    let token = oauth.get("accessToken").and_then(Value::as_str).ok_or(FetchError::NoToken("No Claude Code sign-in"))?;
    if oauth.get("expiresAt").and_then(Value::as_u64).is_some_and(|exp| exp < now_ms()) {
        return Err(FetchError::NoToken("Token expired: use Claude Code to renew it"));
    }
    Ok(token.to_string())
}

fn percent(bucket: &Value) -> Option<f64> {
    if let Some(v) = bucket.get("utilization").and_then(Value::as_f64) {
        return Some(v.clamp(0.0, 100.0));
    }
    ["percent_used", "used_percent", "percent"].iter().find_map(|k| bucket.get(*k).and_then(Value::as_f64)).map(|v| v.clamp(0.0, 100.0))
}

fn bucket_label(key: &str) -> String {
    match key {
        "five_hour" => "5-hour limit".into(),
        "seven_day" => "Weekly limit · all models".into(),
        "seven_day_opus" => "Weekly limit · Opus".into(),
        "seven_day_sonnet" => "Weekly limit · Sonnet".into(),
        "seven_day_oauth_apps" => "Weekly limit · OAuth apps".into(),
        other => other.replace('_', " "),
    }
}

/// Keeps only windows the response actually reports (null buckets are skipped).
pub fn parse_usage(body: &Value) -> (Vec<Quota>, Option<Value>) {
    let mut quotas = Vec::new();
    if let Some(map) = body.as_object() {
        for (key, bucket) in map {
            if !(key.starts_with("five_hour") || key.starts_with("seven_day")) || !bucket.is_object() {
                continue;
            }
            let Some(used) = percent(bucket) else { continue };
            quotas.push(Quota {
                id: key.clone(),
                label: bucket_label(key),
                used,
                window_minutes: None,
                resets_at: bucket.get("resets_at").and_then(Value::as_str).and_then(parse_iso_ms),
            });
        }
    }
    for limit in body.get("limits").and_then(Value::as_array).into_iter().flatten() {
        if limit.get("kind").and_then(Value::as_str) != Some("weekly_scoped") {
            continue;
        }
        let Some(name) = limit.pointer("/scope/model/display_name").and_then(Value::as_str) else { continue };
        let label = format!("Weekly limit · {name}");
        if quotas.iter().any(|q| q.label.eq_ignore_ascii_case(&label)) {
            continue;
        }
        let Some(used) = percent(limit) else { continue };
        quotas.push(Quota {
            id: format!("weekly_scoped_{}", name.to_ascii_lowercase()),
            label,
            used,
            window_minutes: None,
            resets_at: limit.get("resets_at").and_then(Value::as_str).and_then(parse_iso_ms),
        });
    }
    let order = |q: &Quota| if q.id == "five_hour" { 0 } else if q.id == "seven_day" { 1 } else { 2 };
    quotas.sort_by_key(order);
    let credits = body.get("extra_usage").filter(|e| e.is_object() && e.get("is_enabled").and_then(Value::as_bool).unwrap_or(false)).map(|e| {
        serde_json::json!({
            "used": e.get("used_credits"), "limit": e.get("monthly_limit"),
            "currency": e.get("currency").and_then(Value::as_str).filter(|c| c.len() == 3),
            "unit": "minor",
        })
    });
    (quotas, credits)
}

fn fetch() -> Result<(Vec<Quota>, Option<Value>), FetchError> {
    let token = read_oauth_access_token()?;
    let response = ureq::get(OAUTH_USAGE_URL)
        .timeout(Duration::from_secs(8))
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", OAUTH_USAGE_BETA)
        .set("User-Agent", "icue-edge-companion")
        .call();
    let body = match response {
        Ok(r) => String::from_utf8(read_limited(r.into_reader(), 2 * 1024 * 1024).map_err(|_| FetchError::Parse)?).map_err(|_| FetchError::Parse)?,
        Err(ureq::Error::Status(429, r)) => {
            let retry = r.header("retry-after").and_then(|v| v.trim().parse::<u64>().ok()).map(|s| s * 1000).unwrap_or(5 * MIN);
            return Err(FetchError::RateLimited(retry.clamp(MIN, 60 * MIN)));
        }
        Err(ureq::Error::Status(401 | 403, _)) => return Err(FetchError::Denied),
        Err(_) => return Err(FetchError::Network),
    };
    let value: Value = serde_json::from_str(&body).map_err(|_| FetchError::Parse)?;
    let parsed = parse_usage(&value);
    if parsed.0.is_empty() {
        return Err(FetchError::Parse);
    }
    Ok(parsed)
}

/// 60 s while Claude is active, 10 min when idle; 429 honours Retry-After (default 5 min).
pub fn run_quota(shared: Shared) {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = now_ms();
        let due = {
            let mut st = lock(&shared);
            let active = st.saved.claude.sessions.values().any(|s| s.last_event + ACTIVE_MS > now);
            let forced = std::mem::take(&mut st.force_refresh_claude) && st.claude.quota_backoff == 0;
            let due = !st.paused && (forced || now >= st.claude.next_quota_try);
            if due {
                st.claude.next_quota_try = now + if active { MIN } else { 10 * MIN };
                st.claude.refreshing = true;
            }
            due
        };
        if !due {
            continue;
        }
        let result = fetch();
        update(&shared, |st| {
            st.claude.refreshing = false;
            let now = now_ms();
            match result {
                Ok((quotas, credits)) => {
                    let old = std::mem::take(&mut st.saved.claude.quota.quotas);
                    for q in &quotas {
                        if let Some(o) = old.iter().find(|o| o.id == q.id) {
                            if matches!((o.resets_at, q.resets_at), (Some(a), Some(b)) if b > a + MIN) && q.used < o.used {
                                st.claude.event(now, format!("{} reset (reported value: {}%)", q.label, q.used.round()));
                            }
                        }
                    }
                    st.saved.claude.set_quota(QuotaSet { quotas, observed_at: now, src: QUOTA_SRC.into(), credits });
                    st.claude.quota_error = None;
                    st.claude.quota_backoff = 0;
                }
                Err(e) => {
                    let (state, reason, wait) = match e {
                        FetchError::NoToken(msg) => ("error", format!("Quotas: {msg}"), 5 * MIN),
                        FetchError::Denied => ("error", "Quotas: access denied (401/403). Sign in again in Claude Code.".into(), 15 * MIN),
                        FetchError::RateLimited(ms) => ("stale", format!("Rate limited by the source: retrying in {} min", ms.div_ceil(MIN)), ms),
                        FetchError::Network => ("stale", "Quotas: source unreachable".into(), 2 * MIN),
                        FetchError::Parse => ("stale", "Quotas: unrecognized response".into(), 10 * MIN),
                    };
                    st.claude.quota_backoff = wait;
                    st.claude.next_quota_try = now + wait;
                    st.claude.quota_error = Some((state.into(), reason, now));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::store;

    fn line(id: &str, req: &str, input: u64, output: u64, read: u64, write: u64, ts: &str, model: &str) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"s-1","entrypoint":"cli","isSidechain":false,"requestId":"{req}","timestamp":"{ts}","message":{{"id":"{id}","model":"{model}","content":[{{"type":"text","text":"secret prompt text"}}],"usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":{read},"cache_creation_input_tokens":{write}}}}}}}"#
        )
    }

    fn feed(shared: &Shared, lines: &[String]) {
        apply(shared, "f", lines.iter().filter_map(|l| parse_line(l)).collect(), 0);
    }

    #[test]
    fn repeated_lines_count_once_and_streaming_updates_add_growth() {
        let shared = store::tests::load_for_test();
        feed(&shared, &[
            line("m1", "r1", 2, 10, 1000, 50, "2026-09-27T10:00:00Z", "opus"),
            line("m1", "r1", 2, 10, 1000, 50, "2026-09-27T10:00:01Z", "opus"),
            line("m1", "r1", 2, 169, 1000, 50, "2026-09-27T10:00:02Z", "opus"),
            line("m2", "r2", 3, 20, 2000, 0, "2026-09-27T10:01:00Z", "opus"),
        ]);
        // Restart replaying the same file: nothing new.
        feed(&shared, &[line("m1", "r1", 2, 169, 1000, 50, "2026-09-27T10:00:02Z", "opus")]);
        let st = lock(&shared);
        let t: Tok = st.saved.claude.hours.values().fold(Tok::default(), |mut a, t| { a.add(t); a });
        assert_eq!((t.input, t.output, t.cache_read, t.cache_write), (5, 189, 3000, 50));
        let s = st.saved.claude.sessions.get("s-1").unwrap();
        assert_eq!(s.context_used, Some(3 + 2000));
        assert_eq!(s.context_cap, None);
        assert_eq!(s.client, "CLI");
    }

    #[test]
    fn model_change_is_an_event() {
        let shared = store::tests::load_for_test();
        feed(&shared, &[line("m1", "r1", 1, 1, 0, 0, "2026-09-27T10:00:00Z", "sonnet"), line("m2", "r2", 1, 1, 0, 0, "2026-09-27T10:05:00Z", "opus")]);
        assert!(lock(&shared).claude.events.iter().any(|(_, t)| t.contains("sonnet → opus")));
    }

    #[test]
    fn oauth_usage_keeps_reported_windows_only() {
        let body: Value = serde_json::from_str(r#"{"five_hour":{"utilization":42,"resets_at":"2026-09-27T14:45:00Z"},"seven_day":{"utilization":61,"resets_at":"2026-09-30T16:32:00Z"},"seven_day_opus":null,"limits":[{"kind":"weekly_scoped","percent":18,"scope":{"model":{"display_name":"Fable"}}}],"extra_usage":null}"#).unwrap();
        let (q, credits) = parse_usage(&body);
        let labels: Vec<_> = q.iter().map(|q| q.label.as_str()).collect();
        assert_eq!(labels, ["5-hour limit", "Weekly limit · all models", "Weekly limit · Fable"]);
        assert_eq!(q[0].used, 42.0);
        assert!(credits.is_none());
    }

    #[test]
    fn snapshot_has_no_conversation_text() {
        let shared = store::tests::load_for_test();
        feed(&shared, &[line("m1", "r1", 1, 1, 0, 0, "2026-09-27T10:00:00Z", "opus")]);
        let text = store::snapshot(&lock(&shared)).to_string();
        assert!(!text.contains("secret prompt text"));
        assert!(!text.contains("s-1\""));
    }
}
