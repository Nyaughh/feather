//! The file tree: directories are read lazily when expanded, and the visible
//! rows are kept as a flat list so the sidebar can virtualise them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct Node {
    pub name: String,
    /// Path relative to the root, with `/` separators.
    pub rel: String,
    pub is_dir: bool,
    pub depth: u16,
    pub expanded: bool,
    children: Vec<usize>,
}

pub struct Tree {
    root: PathBuf,
    nodes: Vec<Node>,
    top: Vec<usize>,
    /// Indices into `nodes` of the rows currently visible, in display order.
    flat: Vec<usize>,
}

impl Tree {
    pub fn new(root: &Path) -> Self {
        let mut tree = Self {
            root: root.to_owned(),
            nodes: Vec::new(),
            top: Vec::new(),
            flat: Vec::new(),
        };
        tree.top = tree.read_dir("", 0);
        tree.rebuild_flat();
        tree
    }

    pub fn len(&self) -> usize {
        self.flat.len()
    }

    pub fn row(&self, i: usize) -> &Node {
        &self.nodes[self.flat[i]]
    }

    /// Expands or collapses the directory at visible row `i`.
    pub fn toggle(&mut self, i: usize) {
        let idx = self.flat[i];
        if !self.nodes[idx].is_dir {
            return;
        }
        if self.nodes[idx].expanded {
            self.nodes[idx].expanded = false;
        } else {
            self.expand(idx);
        }
        self.rebuild_flat();
    }

    fn expand(&mut self, idx: usize) {
        let rel = self.nodes[idx].rel.clone();
        let depth = self.nodes[idx].depth + 1;
        let children = self.read_dir(&rel, depth);
        self.nodes[idx].children = children;
        self.nodes[idx].expanded = true;
    }

    /// Re-reads everything from disk, keeping the same directories open.
    pub fn refresh(&mut self) {
        let expanded: HashSet<String> = self
            .nodes
            .iter()
            .filter(|n| n.expanded)
            .map(|n| n.rel.clone())
            .collect();
        self.nodes.clear();
        self.top = self.read_dir("", 0);
        let mut stack = self.top.clone();
        while let Some(idx) = stack.pop() {
            if self.nodes[idx].is_dir && expanded.contains(&self.nodes[idx].rel) {
                self.expand(idx);
                stack.extend(self.nodes[idx].children.iter().copied());
            }
        }
        self.rebuild_flat();
    }

    fn rebuild_flat(&mut self) {
        self.flat.clear();
        let mut stack: Vec<usize> = self.top.iter().rev().copied().collect();
        while let Some(idx) = stack.pop() {
            self.flat.push(idx);
            let node = &self.nodes[idx];
            if node.expanded {
                stack.extend(node.children.iter().rev().copied());
            }
        }
    }

    fn read_dir(&mut self, rel: &str, depth: u16) -> Vec<usize> {
        let dir = if rel.is_empty() {
            self.root.clone()
        } else {
            self.root.join(rel)
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut found: Vec<(bool, String)> = entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == ".git" || name == ".DS_Store" {
                    return None;
                }
                let file_type = entry.file_type().ok()?;
                let is_dir = if file_type.is_symlink() {
                    std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
                } else {
                    file_type.is_dir()
                };
                Some((is_dir, name))
            })
            .collect();
        // Folders first, then case-insensitive by name.
        found.sort_by_cached_key(|(is_dir, name)| (!*is_dir, name.to_lowercase()));

        found
            .into_iter()
            .map(|(is_dir, name)| {
                let rel = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                self.nodes.push(Node {
                    name,
                    rel,
                    is_dir,
                    depth,
                    expanded: false,
                    children: Vec::new(),
                });
                self.nodes.len() - 1
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn names(tree: &Tree) -> Vec<String> {
        (0..tree.len()).map(|i| tree.row(i).rel.clone()).collect()
    }

    #[test]
    fn lists_sorts_expands_and_refreshes() {
        let dir = std::env::temp_dir().join(format!("feather-tree-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/ui")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("b.txt"), "").unwrap();
        fs::write(dir.join("A.txt"), "").unwrap();
        fs::write(dir.join("src/main.rs"), "").unwrap();
        fs::write(dir.join("src/ui/mod.rs"), "").unwrap();

        let mut tree = Tree::new(&dir);
        assert_eq!(names(&tree), ["src", "A.txt", "b.txt"]);

        tree.toggle(0);
        assert_eq!(
            names(&tree),
            ["src", "src/ui", "src/main.rs", "A.txt", "b.txt"]
        );
        assert_eq!(tree.row(1).depth, 1);
        tree.toggle(1);
        assert_eq!(tree.row(2).rel, "src/ui/mod.rs");

        // New files appear on refresh and open folders stay open.
        fs::write(dir.join("src/ui/new.rs"), "").unwrap();
        tree.refresh();
        assert_eq!(
            names(&tree),
            [
                "src",
                "src/ui",
                "src/ui/mod.rs",
                "src/ui/new.rs",
                "src/main.rs",
                "A.txt",
                "b.txt"
            ]
        );

        tree.toggle(0);
        assert_eq!(names(&tree), ["src", "A.txt", "b.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
