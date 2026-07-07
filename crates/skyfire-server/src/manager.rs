use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skyfire_hls::{HlsConfig, HlsSession};

/// Owns one `HlsSession` per slug, lazily started from `<dir>/<slug>.ts`.
pub struct Manager {
    dir: PathBuf,
    live: Vec<String>,
    sessions: Mutex<HashMap<String, HlsSession>>,
}

#[expect(dead_code)]
impl Manager {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>, live: Vec<String>) -> Self {
        Self {
            dir: dir.into(),
            live,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Every `<slug>.ts` file in the fixtures dir, sorted.
    #[must_use]
    pub fn slugs(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().and_then(|x| x.to_str()) == Some("ts"))
                    .then(|| p.file_stem().and_then(|s| s.to_str()).map(String::from))
                    .flatten()
            })
            .collect();
        out.sort();
        out
    }

    fn ensure(&self, slug: &str) -> bool {
        let mut map = self.sessions.lock().unwrap();
        if map.contains_key(slug) {
            return true;
        }
        let path = self.dir.join(format!("{slug}.ts"));
        let Ok(data) = std::fs::read(&path) else {
            return false;
        };
        let mut session = if self.live.iter().any(|s| s == slug) {
            HlsSession::new(HlsConfig::rolling(6))
        } else {
            HlsSession::new(HlsConfig::vod())
        };
        // VOD: feed the whole file up front (deterministic). Live mode feeds
        // incrementally on a timer — added in Task 6; for now feed fully.
        session.feed(&data);
        session.finish();
        map.insert(slug.to_string(), session);
        true
    }

    #[must_use]
    pub fn playlist(&self, slug: &str) -> Option<String> {
        if !self.ensure(slug) {
            return None;
        }
        let map = self.sessions.lock().unwrap();
        map.get(slug).map(|s| s.playlist())
    }

    #[must_use]
    pub fn is_ready(&self, slug: &str) -> bool {
        if !self.ensure(slug) {
            return false;
        }
        let map = self.sessions.lock().unwrap();
        map.get(slug).is_some_and(HlsSession::is_ready)
    }

    #[must_use]
    pub fn segment(&self, slug: &str, name: &str) -> Option<Arc<Vec<u8>>> {
        if !self.ensure(slug) {
            return None;
        }
        let map = self.sessions.lock().unwrap();
        map.get(slug).and_then(|s| s.segment(name))
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_ts_slugs_from_fixtures_dir() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let m = Manager::new(dir, vec![]);
        let slugs = m.slugs();
        assert!(
            slugs.iter().any(|s| s == "h264-25fps"),
            "must list h264-25fps, got {slugs:?}"
        );
    }

    #[test]
    fn serves_vod_playlist_and_segments_for_a_real_fixture() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let m = Manager::new(dir, vec![]);
        assert!(m.is_ready("france2-8s"), "france2-8s must become ready");
        let pl = m.playlist("france2-8s").unwrap();
        assert!(pl.contains("#EXT-X-ENDLIST"));
        let first = pl.lines().find(|l| l.ends_with(".ts")).unwrap();
        assert!(m.segment("france2-8s", first).is_some());
        assert!(m.segment("france2-8s", "nope.ts").is_none());
    }
}
