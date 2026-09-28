//! Which of our iCUE widgets are installed, placed on a screen and connected. Read-only: iCUE's own
//! files are scanned for widget ids (`<typeId>com.stealthsrc.…</typeId>`), never modified.
//! XENEON EDGE layouts live in `CUE5\dashlcd\storage`; pump LCD screens in the profiles.
use crate::http::{self, Feed};
use crate::util::now_ms;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// (id suffix, name, screen, what proves it is connected)
const OURS: &[(&str, &str, &str, Link)] = &[
    ("claudeusage", "Claude Usage", "XENEON EDGE", Link::Stream(Feed::Usage)),
    ("codexusage", "Codex Usage", "XENEON EDGE", Link::Stream(Feed::Usage)),
    ("nowplaying", "Now Playing", "XENEON EDGE", Link::Stream(Feed::Media)),
    ("spotify", "Spotify", "XENEON EDGE", Link::Stream(Feed::Spotify)),
    ("windowsmediapump", "Windows Media Pump", "Pump LCD", Link::Relay),
];

#[derive(Clone, Copy)]
enum Link {
    Stream(Feed),
    /// The pump renderer cannot open a connection; it reads the files the relay writes.
    Relay,
}

fn cue_dir() -> Option<PathBuf> {
    Some(Path::new(&std::env::var_os("APPDATA")?).join(r"Corsair\CUE5"))
}

/// Our widget ids (suffix after `com.stealthsrc.`) mentioned in a text.
pub fn ids_in(text: &str) -> Vec<String> {
    const TAG: &str = "<typeId>com.stealthsrc.";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(TAG) {
        rest = &rest[i + TAG.len()..];
        let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if !id.is_empty() && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn ids_in_file(path: &Path) -> Vec<String> {
    std::fs::read(path).map(|b| ids_in(&String::from_utf8_lossy(&b))).unwrap_or_default()
}

pub fn inventory() -> Value {
    let Some(cue) = cue_dir() else { return json!([]) };
    let on_edge = ids_in_file(&cue.join(r"dashlcd\storage"));
    let on_pump: Vec<String> = std::fs::read_dir(cue.join("profiles"))
        .map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "cueprofiledata")).flat_map(|e| ids_in_file(&e.path())).collect())
        .unwrap_or_default();
    let relay_fresh = crate::media::relay::written_at().is_some_and(|t| t + 10_000 > now_ms());
    OURS.iter()
        .map(|&(id, name, screen, link)| {
            let installed = cue.join(r"html_widgets\com\stealthsrc").join(id).join("manifest.json").is_file();
            let placed = if screen == "Pump LCD" { on_pump.iter().any(|x| x == id) } else { on_edge.iter().any(|x| x == id) };
            let live = placed
                && match link {
                    Link::Stream(feed) => http::feed_streams(feed) > 0,
                    Link::Relay => relay_fresh,
                };
            json!({ "name": name, "screen": screen, "installed": installed, "placed": placed, "live": live })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_our_ids_once_and_ignores_others() {
        let text = "<typeId>com.corsair.digitalclock1</typeId><typeId>com.stealthsrc.spotify</typeId>\n<typeId>com.stealthsrc.claudeusage</typeId><typeId>com.stealthsrc.spotify</typeId>";
        assert_eq!(ids_in(text), ["spotify", "claudeusage"]);
    }
}
