use crossbeam_channel::{Receiver, unbounded};
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

pub struct SearchResult {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub accessed: Option<SystemTime>,
}

pub struct SearchHandle {
    pub rx: Receiver<SearchResult>,
    pub cancel: Arc<AtomicBool>,
}

/// Simple glob matching: `*` = any chars, `?` = one char. No deps needed.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (plen, tlen) = (p.len(), t.len());
    let (mut pi, mut ti) = (0, 0);
    let (mut star_p, mut star_t) = (usize::MAX, 0);

    while ti < tlen {
        if pi < plen && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < plen && p[pi] == b'*' {
            star_p = pi;
            star_t = ti;
            pi += 1;
        } else if star_p != usize::MAX {
            pi = star_p + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < plen && p[pi] == b'*' {
        pi += 1;
    }
    pi == plen
}

/// Returns true if the query contains wildcard characters.
pub fn is_glob(query: &str) -> bool {
    query.contains('*') || query.contains('?')
}

pub fn search(root: &Path, query: &str, case_sensitive: bool) -> SearchHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = unbounded::<SearchResult>();

    let root = root.to_path_buf();
    let query = query.to_owned();
    let cancel_clone = cancel.clone();

    std::thread::spawn(move || {
        let is_glob_query = is_glob(&query);
        let prepared = if case_sensitive { query.clone() } else { query.to_lowercase() };

        WalkBuilder::new(&root)
            .threads(std::thread::available_parallelism().map_or(8, |n| n.get().min(8)))
            .hidden(true)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(true)
            .build_parallel()
            .run(|| {
                let tx = tx.clone();
                let prepared = prepared.clone();
                let cancel = cancel_clone.clone();
                let is_glob_query = is_glob_query;
                let case_sensitive = case_sensitive;

                Box::new(move |result| {
                    if cancel.load(Ordering::Relaxed) {
                        return ignore::WalkState::Quit;
                    }

                    let entry = match result {
                        Ok(e) => e,
                        Err(_) => return ignore::WalkState::Continue,
                    };

                    let name = entry.file_name().to_string_lossy();
                    let hay = if case_sensitive { name.to_string() } else { name.to_lowercase() };

                    let matches = if is_glob_query {
                        glob_matches(&prepared, &hay)
                    } else {
                        hay.contains(&*prepared)
                    };

                    if matches {
                        let is_dir = entry.file_type().map_or(false, |ft| ft.is_dir());
                        // Only stat for files (need size + accessed)
                        let (size, accessed) = if !is_dir {
                            entry.metadata().map_or((0, None), |m| {
                                (m.len(), m.accessed().ok())
                            })
                        } else {
                            (0, None)
                        };
                        let _ = tx.send(SearchResult {
                            path: entry.into_path(),
                            is_dir,
                            size,
                            accessed,
                        });
                    }

                    ignore::WalkState::Continue
                })
            });
    });

    SearchHandle { rx, cancel }
}

/// Drains available results from the channel (non-blocking, capped per frame to stay responsive).
pub fn drain_results(handle: &SearchHandle, buf: &mut Vec<SearchResult>) {
    for _ in 0..2000 {
        match handle.rx.try_recv() {
            Ok(r) => buf.push(r),
            Err(_) => break,
        }
    }
}

/// Returns true if the search is done (channel closed, all senders dropped).
pub fn is_done(handle: &SearchHandle) -> bool {
    matches!(handle.rx.try_recv(), Err(crossbeam_channel::TryRecvError::Disconnected))
}
