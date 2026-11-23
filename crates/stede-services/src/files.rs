// SPDX-License-Identifier: GPL-3.0-or-later
//! Folder listings for the file tree.

use std::io::Read;
use std::path::Path;

use stede_core::io::{Document, Entry};
use stede_core::render::{Block, BlockKind, LineRange, RenderModel};
use stede_markdown::{Flavor, line_count, render};

/// Files larger than this aren't opened: the reader holds the whole file.
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// Extensions read as Markdown; other text files show as one code block.
const MARKDOWN: &[&str] = &["md", "markdown", "mdown", "mkd", "mkdn", "mdx", "mdtext"];

/// Reads a file and builds its render model. Markdown files are parsed in
/// `flavor` (GFM if it's `None` or unknown, since config already reported
/// an unknown name); other text files become a single code block, labeled
/// by their extension. Binary files and very large ones are refused.
pub fn open_file(path: &Path, flavor: Option<&str>) -> Result<Document, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size > MAX_FILE_BYTES {
        return Err(format!(
            "too large to open ({} MiB; the limit is {} MiB)",
            size / (1024 * 1024),
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    // A NUL near the start is the usual sign of a binary file.
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err("it looks like a binary file".into());
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let markdown = MARKDOWN.contains(&ext.as_str());
    let model = if markdown {
        let flavor = flavor
            .and_then(|f| f.parse::<Flavor>().ok())
            .unwrap_or_default();
        render(&text, flavor)
    } else {
        RenderModel {
            blocks: vec![Block {
                kind: BlockKind::Code {
                    lang: (!ext.is_empty()).then(|| ext.clone()),
                    text: text
                        .trim_end_matches(['\n', '\r'])
                        .replace("\r\n", "\n")
                        .replace('\r', "\n"),
                },
                source: LineRange {
                    start: 1,
                    end: line_count(&text),
                },
                containers: Vec::new(),
            }],
        }
    };
    Ok(Document {
        path: path.to_path_buf(),
        text,
        model,
        markdown,
    })
}

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

    /// A fresh folder per test: tests run in parallel.
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stede-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folders_first_ignored_and_hidden_left_out() {
        let dir = temp("list");
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
    fn opens_markdown_and_other_text_and_refuses_binaries() {
        let dir = temp("open");
        std::fs::write(dir.join("a.md"), "# Hi\n\nbody\n").unwrap();
        std::fs::write(dir.join("b.rs"), "fn main() {}\r\n").unwrap();
        std::fs::write(dir.join("c.bin"), b"PK\0\x03junk").unwrap();
        let md = open_file(&dir.join("a.md"), None).unwrap();
        assert!(md.markdown);
        assert_eq!(md.model.blocks.len(), 2);
        let rs = open_file(&dir.join("b.rs"), Some("gfm")).unwrap();
        assert!(!rs.markdown);
        assert_eq!(
            rs.model.blocks[0].kind,
            BlockKind::Code {
                lang: Some("rs".into()),
                text: "fn main() {}".into()
            }
        );
        assert!(
            open_file(&dir.join("c.bin"), None)
                .unwrap_err()
                .contains("binary")
        );
        assert!(open_file(&dir.join("none.md"), None).is_err());
        // A CommonMark flavor turns tables off.
        std::fs::write(dir.join("t.md"), "| a |\n|---|\n| b |\n").unwrap();
        let gfm = open_file(&dir.join("t.md"), None).unwrap();
        let cm = open_file(&dir.join("t.md"), Some("commonmark")).unwrap();
        assert!(matches!(gfm.model.blocks[0].kind, BlockKind::Table(_)));
        assert!(matches!(cm.model.blocks[0].kind, BlockKind::Paragraph(_)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_folders_are_errors() {
        assert!(list_dir(Path::new("/no/such/folder/here")).is_err());
    }
}
