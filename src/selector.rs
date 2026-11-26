use crate::download::{cmp_natural_ci, ScannedFile};
use std::collections::BTreeMap;

pub(crate) struct SelectableFile<T = ScannedFile> {
    pub(crate) scanned: T,
    path: String,
    search_path: String,
    pub(crate) checked: bool,
}

impl<T> SelectableFile<T> {
    pub(crate) fn new(scanned: T, path: String) -> Self {
        Self {
            search_path: path.to_lowercase(),
            scanned,
            path,
            checked: true,
        }
    }
}

pub(crate) struct Selection<T = ScannedFile> {
    pub(crate) items: Vec<SelectableFile<T>>,
    pub(crate) root: String,
    pub(crate) folder: String,
    pub(crate) selected_only: bool,
}

pub(crate) struct Row<'a> {
    pub(crate) path: &'a str,
    pub(crate) name: &'a str,
    pub(crate) file_index: Option<usize>,
    pub(crate) total: usize,
    pub(crate) selected: usize,
}

fn name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn under_folder<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    if folder.is_empty() {
        Some(path)
    } else {
        path.strip_prefix(folder)?.strip_prefix('/')
    }
}

impl<T> Selection<T> {
    pub(crate) fn new(items: Vec<SelectableFile<T>>) -> Self {
        // The engine prefixes all paths in a folder scan with the shared root name.
        let root = items
            .first()
            .and_then(|item| item.path.split_once('/'))
            .map(|(root, _)| root)
            .filter(|root| {
                items
                    .iter()
                    .all(|item| under_folder(&item.path, root).is_some())
            })
            .unwrap_or_default()
            .to_owned();
        Self {
            items,
            folder: root.clone(),
            root,
            selected_only: false,
        }
    }

    pub(crate) fn rows(&self, filter: &str, ascending: bool) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        let mut folders = BTreeMap::new();
        for (index, item) in self.items.iter().enumerate() {
            let matches = item.search_path.contains(filter);
            let relative = if self.selected_only {
                if !item.checked {
                    continue;
                }
                name(&item.path)
            } else {
                let Some(relative) = under_folder(&item.path, &self.folder) else {
                    continue;
                };
                relative
            };

            if let Some((child, _)) = relative.split_once('/') {
                let prefix_len = item.path.len() - relative.len() + child.len();
                let (row, any_match) = folders.entry(&item.path[..prefix_len]).or_insert((
                    Row {
                        path: &item.path[..prefix_len],
                        name: child,
                        file_index: None,
                        total: 0,
                        selected: 0,
                    },
                    false,
                ));
                row.total += 1;
                row.selected += usize::from(item.checked);
                *any_match |= matches;
            } else if matches {
                rows.push(Row {
                    path: &item.path,
                    name: relative,
                    file_index: Some(index),
                    total: 1,
                    selected: usize::from(item.checked),
                });
            }
        }
        rows.extend(
            folders
                .into_values()
                .filter_map(|(row, matches)| matches.then_some(row)),
        );
        rows.sort_by(|a, b| {
            let by_name =
                cmp_natural_ci(a.name, b.name).then_with(|| cmp_natural_ci(a.path, b.path));
            (b.selected > 0).cmp(&(a.selected > 0)).then_with(|| {
                if ascending {
                    by_name
                } else {
                    by_name.reverse()
                }
            })
        });
        rows
    }

    pub(crate) fn toggle_file(&mut self, index: usize) {
        if let Some(item) = self.items.get_mut(index) {
            item.checked = !item.checked;
        }
    }

    pub(crate) fn toggle_folder(&mut self, folder: &str) {
        let checked = self
            .items
            .iter()
            .any(|item| !item.checked && under_folder(&item.path, folder).is_some());
        for item in &mut self.items {
            if under_folder(&item.path, folder).is_some() {
                item.checked = checked;
            }
        }
    }

    pub(crate) fn open_folder(&mut self, folder: String) {
        if under_folder(&folder, &self.folder)
            .is_some_and(|suffix| !suffix.is_empty() && !suffix.contains('/'))
            && self
                .items
                .iter()
                .any(|item| under_folder(&item.path, &folder).is_some())
        {
            self.folder = folder;
        }
    }

    pub(crate) fn go_up(&mut self) {
        if self.folder != self.root {
            let parent = self.folder.rsplit_once('/').map(|(parent, _)| parent);
            self.folder.truncate(parent.map_or(0, str::len));
        }
    }

    pub(crate) fn set_visible(&mut self, filter: &str, checked: bool) {
        for item in &mut self.items {
            let in_scope = if self.selected_only {
                item.checked
            } else {
                under_folder(&item.path, &self.folder).is_some()
            };
            if in_scope && item.search_path.contains(filter) {
                item.checked = checked;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Selection<()> {
        Selection::new(
            [
                "v12/archive/sub/part10.zip",
                "v12/archive/part2.zip",
                "v12/archive-extra/part2.zip",
                "v12/addon/addon.txt",
                "v12/alpha.txt",
                "v12/Zeta.txt",
            ]
            .into_iter()
            .map(|path| SelectableFile::new((), path.into()))
            .collect(),
        )
    }

    #[test]
    fn navigates_and_selects_folder_descendants_without_touching_siblings() {
        let mut selection = example();
        assert_eq!(selection.folder, "v12");
        assert_eq!(
            selection
                .rows("", true)
                .iter()
                .map(|row| row.name)
                .collect::<Vec<_>>(),
            ["addon", "alpha.txt", "archive", "archive-extra", "Zeta.txt"]
        );

        selection.toggle_folder("v12/archive");
        assert!(!selection.items[0].checked);
        assert!(!selection.items[1].checked);
        assert!(selection.items[2].checked);
        selection.open_folder("v12/archive".into());
        selection.toggle_file(1);
        let archive = selection.rows("", true);
        assert_eq!(
            archive.iter().map(|row| row.name).collect::<Vec<_>>(),
            ["part2.zip", "sub"]
        );
        selection.go_up();
        let root = selection.rows("part10", true);
        assert_eq!(root.len(), 1);
        assert_eq!(
            (root[0].name, root[0].selected, root[0].total),
            ("archive", 1, 2)
        );
        selection.toggle_folder("v12/archive");
        assert!(selection.items[0].checked);
    }

    #[test]
    fn keeps_selected_first_in_both_directions_and_can_review_them_globally() {
        let mut selection = example();
        selection.set_visible("", false);
        selection.toggle_file(5);
        selection.toggle_file(4);
        for (ascending, expected) in [
            (true, ["alpha.txt", "Zeta.txt"]),
            (false, ["Zeta.txt", "alpha.txt"]),
        ] {
            assert_eq!(
                selection
                    .rows("", ascending)
                    .iter()
                    .take(2)
                    .map(|row| row.name)
                    .collect::<Vec<_>>(),
                expected
            );
        }
        selection.toggle_file(1);
        selection.open_folder("v12/addon".into());
        selection.set_visible("", true);
        selection.selected_only = true;
        let picked = selection.rows("part2", true);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].path, "v12/archive/part2.zip");
        selection.set_visible("part2", false);
        assert!(!selection.items[1].checked);
        assert!(selection.items[3].checked);
        assert!(selection.items[4].checked);
    }
}
