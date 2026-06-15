//! Filesystem path autocomplete for text fields, modeled on bosun's
//! new-session path picker: directory listing filtered by the trailing
//! segment, dirs-first ordering, `~` expansion, and Tab longest-common-prefix
//! completion. Keeps directory navigation keyboard-driven instead of a blind
//! text field.

#[derive(Clone)]
pub struct PathEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Split a path into (directory-with-trailing-slash, trailing-segment).
pub fn split_path(path: &str) -> (String, String) {
    if path.is_empty() {
        return (String::new(), String::new());
    }
    if path.ends_with('/') {
        return (path.to_string(), String::new());
    }
    match path.rfind('/') {
        Some(idx) => (path[..=idx].to_string(), path[idx + 1..].to_string()),
        None => (String::new(), path.to_string()),
    }
}

/// Expand a leading `~`/`~/` to `$HOME` for filesystem lookups.
pub fn expand_tilde(path: &str) -> String {
    if path == "~" {
        return std::env::var("HOME").unwrap_or_default();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        return format!("{home}/{rest}");
    }
    path.to_string()
}

/// Entries in the directory implied by `path` whose names start with its
/// trailing segment. Dirs first, then files, alphabetical within each group.
/// Hidden entries are excluded unless the typed prefix itself starts with `.`.
pub fn read_dir_filtered(path: &str, limit: usize) -> Vec<PathEntry> {
    let (dir, prefix) = split_path(path);
    let lookup = if dir.is_empty() {
        ".".to_string()
    } else {
        expand_tilde(&dir)
    };
    let Ok(read) = std::fs::read_dir(&lookup) else {
        return Vec::new();
    };
    let show_hidden = prefix.starts_with('.');
    let mut out: Vec<PathEntry> = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        if !name.starts_with(&prefix) {
            continue;
        }
        let is_dir = entry.file_type().ok().map(|t| t.is_dir()).unwrap_or(false);
        out.push(PathEntry { name, is_dir });
    }
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    out.truncate(limit);
    out
}

/// Character-wise longest common prefix (Unicode-safe).
pub fn longest_common_prefix(strs: &[&str]) -> String {
    if strs.is_empty() {
        return String::new();
    }
    let mut prefix: Vec<char> = strs[0].chars().collect();
    for s in &strs[1..] {
        let common = prefix
            .iter()
            .zip(s.chars())
            .take_while(|(a, b)| **a == *b)
            .count();
        prefix.truncate(common);
        if prefix.is_empty() {
            break;
        }
    }
    prefix.into_iter().collect()
}

/// Replace the trailing segment of `path` with `entry.name`, adding a trailing
/// slash for directories so the next listing dives in.
pub fn commit_entry(path: &str, entry: &PathEntry) -> String {
    let (dir, _) = split_path(path);
    let mut new_path = format!("{dir}{}", entry.name);
    if entry.is_dir {
        new_path.push('/');
    }
    new_path
}
