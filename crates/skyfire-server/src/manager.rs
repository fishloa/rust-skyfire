use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skyfire_hls::{HlsConfig, HlsSession};

/// Owns one `HlsSession` per slug, lazily started from `<dir>/<slug>.ts`.
pub struct Manager {
    dir: PathBuf,
    live: Vec<String>,
    sessions: Mutex<HashMap<String, HlsSession>>,
    live_files: Mutex<HashMap<String, (Vec<u8>, usize)>>,
}

impl Manager {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>, live: Vec<String>) -> Self {
        Self {
            dir: dir.into(),
            live,
            sessions: Mutex::new(HashMap::new()),
            live_files: Mutex::new(HashMap::new()),
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
        // Live slugs are fed incrementally via feed_live_step — do NOT feed fully.
        if self.live.iter().any(|s| s == slug) {
            return self.sessions.lock().unwrap().contains_key(slug);
        }
        let mut map = self.sessions.lock().unwrap();
        if map.contains_key(slug) {
            return true;
        }
        let path = self.dir.join(format!("{slug}.ts"));
        let Ok(data) = std::fs::read(&path) else {
            return false;
        };
        let mut session = HlsSession::new(HlsConfig::vod());
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

    /// Feed the next `step` bytes of a live slug's file into its session.
    /// No-op once EOF is reached. Creates the rolling session on first call.
    pub fn feed_live_step(&self, slug: &str, step: usize) {
        if !self.live.iter().any(|s| s == slug) {
            // Non-live slugs are served whole via ensure().
            let _ = self.ensure(slug);
            return;
        }
        {
            // Lazily load the file + create the session.
            let mut files = self.live_files.lock().unwrap();
            if !files.contains_key(slug) {
                let path = self.dir.join(format!("{slug}.ts"));
                let Ok(data) = std::fs::read(&path) else {
                    return;
                };
                files.insert(slug.to_string(), (data, 0));
                self.sessions
                    .lock()
                    .unwrap()
                    .entry(slug.to_string())
                    .or_insert_with(|| HlsSession::new(HlsConfig::rolling(6)));
            }
        }
        let mut files = self.live_files.lock().unwrap();
        let Some((data, cursor)) = files.get_mut(slug) else {
            return;
        };
        if *cursor >= data.len() {
            return;
        }
        let end = (*cursor + step).min(data.len());
        let chunk = data[*cursor..end].to_vec();
        *cursor = end;
        let eof = *cursor >= data.len();
        drop(files);
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(s) = sessions.get_mut(slug) {
            s.feed(&chunk);
            if eof {
                s.finish();
            }
        }
    }

    #[must_use]
    pub fn at_eof(&self, slug: &str) -> bool {
        let files = self.live_files.lock().unwrap();
        files.get(slug).is_some_and(|(d, c)| *c >= d.len())
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
