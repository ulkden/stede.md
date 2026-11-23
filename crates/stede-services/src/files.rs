// SPDX-License-Identifier: GPL-3.0-or-later
//! Folder listings for the file tree.

use std::path::Path;

use stede_core::io::Entry;

/// A folder's entries: folders first, then files, each sorted by name
/// ignoring case. Hidden entries (dotfiles) and anything `.gitignore`,
/// `.ignore` or git's excludes leave out are skipped, in or out of a git repo.
/// Symlinks to folders count as folders.
pub fn list_dir(dir: &Path) -> Result<Vec<Entry>, String> {
    // Reading the folder directly first turns "permission denied" and "not a
    // folder" into a clear error; the walker would only skip it.
    std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .parents(true)
        .require_git(false)
        .follow_links(false)
        .build();
    let mut entries = Vec::new();
    for item in walker {
        let Ok(item) = item else {
            continue;
        };
        if item.depth() == 0 {
            continue;
        }
        let path = item.into_path();
        let is_dir = std::fs::metadata(&path).is_ok_and(|m| m.is_dir());
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        entries.push(Entry { name, path, is_dir });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stede-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folders_first_ignored_and_hidden_left_out() {
        let dir = temp();
        for f in ["b.md", "A.md", "secret.log", ".hidden"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        std::fs::create_dir(dir.join("zdocs")).unwrap();
        std::fs::create_dir(dir.join("build")).unwrap();
        std::fs::write(dir.join(".gitignore"), "*.log\nbuild/\n").unwrap();
        let names: Vec<(String, bool)> = list_dir(&dir)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.is_dir))
            .collect();
        assert_eq!(
            names,
            vec![
                ("zdocs".into(), true),
                ("A.md".into(), false),
                ("b.md".into(), false),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_folders_are_errors() {
        assert!(list_dir(Path::new("/no/such/folder/here")).is_err());
    }
}
