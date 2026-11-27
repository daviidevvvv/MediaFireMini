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

    fn matches_filter(&self, filter: &str) -> bool {
        self.search_path.contains(filter)
    }

    fn matches_in_folder(&self, folder: &str, filter: &str) -> bool {
        self.matches_filter(filter) && under_folder(&self.path, folder).is_some()
    }
}

pub(crate) struct Selection<T = ScannedFile> {
    pub(crate) items: Vec<SelectableFile<T>>,
    pub(crate) root: String,
    pub(crate) folder: String,
    pub(crate) selected_only: bool,
    browse_filter: String,
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
            browse_filter: String::new(),
        }
    }

    // Download and summary always use the complete selection, not the current view.
    pub(crate) fn selected_items(&self) -> impl Iterator<Item = &SelectableFile<T>> {
        self.items.iter().filter(|item| item.checked)
    }

    // Enter an unfiltered global review; restore the browsing filter on exit.
    pub(crate) fn toggle_selected_only(&mut self, current_filter: &str) -> String {
        self.selected_only = !self.selected_only;
        if self.selected_only {
            self.browse_filter = current_filter.to_owned();
            String::new()
        } else {
            std::mem::take(&mut self.browse_filter)
        }
    }

    fn is_child_folder(&self, folder: &str) -> bool {
        !self.selected_only
            && under_folder(folder, &self.folder)
                .is_some_and(|suffix| !suffix.is_empty() && !suffix.contains('/'))
    }

    pub(crate) fn rows(&self, filter: &str, ascending: bool) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        let mut folders = BTreeMap::new();
        for (index, item) in self.items.iter().enumerate() {
            if !item.matches_filter(filter) {
                continue;
            }
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
                let row = folders.entry(&item.path[..prefix_len]).or_insert(Row {
                    path: &item.path[..prefix_len],
                    name: child,
                    file_index: None,
                    total: 0,
                    selected: 0,
                });
                row.total += 1;
                row.selected += usize::from(item.checked);
            } else {
                rows.push(Row {
                    path: &item.path,
                    name: relative,
                    file_index: Some(index),
                    total: 1,
                    selected: usize::from(item.checked),
                });
            }
        }
        rows.extend(folders.into_values());
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

    pub(crate) fn toggle_folder(&mut self, folder: &str, filter: &str) {
        if !self.is_child_folder(folder) {
            return;
        }
        let checked = self
            .items
            .iter()
            .any(|item| !item.checked && item.matches_in_folder(folder, filter));
        for item in &mut self.items {
            if item.matches_in_folder(folder, filter) {
                item.checked = checked;
            }
        }
    }

    pub(crate) fn open_folder(&mut self, folder: String) {
        if self.is_child_folder(&folder)
            && self
                .items
                .iter()
                .any(|item| under_folder(&item.path, &folder).is_some())
        {
            self.folder = folder;
        }
    }

    pub(crate) fn go_up(&mut self) {
        if !self.selected_only && self.folder != self.root {
            let parent = self.folder.rsplit_once('/').map(|(parent, _)| parent);
            self.folder.truncate(parent.map_or(0, str::len));
        }
    }

    pub(crate) fn set_visible(&mut self, filter: &str, checked: bool) {
        for item in &mut self.items {
            let in_scope = if self.selected_only {
                item.checked && item.matches_filter(filter)
            } else {
                item.matches_in_folder(&self.folder, filter)
            };
            if in_scope {
                item.checked = checked;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Selection<&'static str> {
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
            .map(|path| SelectableFile::new(path, path.into()))
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

        selection.toggle_folder("v12/archive", "");
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
            ("archive", 0, 1)
        );
        selection.toggle_folder("v12/archive", "part10");
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
        assert_eq!(selection.toggle_selected_only("part2"), "");
        let picked = selection.rows("part2", true);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].path, "v12/archive/part2.zip");
        selection.set_visible("part2", false);
        assert!(!selection.items[1].checked);
        assert!(selection.items[3].checked);
        assert!(selection.items[4].checked);
    }

    fn selected_jobs<'a>(selection: &Selection<&'a str>) -> Vec<&'a str> {
        selection.selected_items().map(|item| item.scanned).collect()
    }

    #[test]
    fn filtered_folder_counts_and_toggles_ignore_hidden_descendants() {
        let mut selection = example();
        selection.set_visible("", false);
        selection.toggle_file(1); // Selected, but hidden by the part10 filter.
        let row = selection.rows("part10", true);
        assert_eq!(row.len(), 1);
        assert_eq!((row[0].selected, row[0].total), (0, 1));

        selection.toggle_folder("v12/archive", "part10");
        let row = selection.rows("part10", true);
        assert_eq!((row[0].selected, row[0].total), (1, 1));
        assert_eq!(
            selected_jobs(&selection),
            ["v12/archive/sub/part10.zip", "v12/archive/part2.zip"]
        );

        selection.toggle_folder("v12/archive", "part10");
        assert_eq!(selected_jobs(&selection), ["v12/archive/part2.zip"]);
        let row = selection.rows("part10", true);
        assert_eq!((row[0].selected, row[0].total), (0, 1));
        let row = selection.rows("", true);
        let archive = row.iter().find(|row| row.path == "v12/archive").unwrap();
        assert_eq!((archive.selected, archive.total), (1, 2));
    }

    #[test]
    fn partial_folder_and_bulk_operations_agree_without_touching_siblings() {
        let mut folder = example();
        let mut bulk = example();
        for selection in [&mut folder, &mut bulk] {
            selection.set_visible("", false);
            selection.toggle_file(0);
            selection.toggle_file(3);
        }
        let rows = folder.rows(".zip", true);
        let archive = rows.iter().find(|row| row.path == "v12/archive").unwrap();
        assert_eq!((archive.selected, archive.total), (1, 2));
        folder.toggle_folder("v12/archive", ".zip");
        bulk.open_folder("v12/archive".into());
        bulk.set_visible(".zip", true);
        assert_eq!(selected_jobs(&folder), selected_jobs(&bulk));
        assert_eq!(
            selected_jobs(&folder),
            [
                "v12/archive/sub/part10.zip",
                "v12/archive/part2.zip",
                "v12/addon/addon.txt"
            ]
        );
        folder.toggle_folder("v12/archive", ".zip");
        bulk.set_visible(".zip", false);
        assert_eq!(selected_jobs(&folder), selected_jobs(&bulk));
        assert_eq!(selected_jobs(&folder), ["v12/addon/addon.txt"]);
    }

    #[test]
    fn folder_name_filter_and_no_match_preserve_unrelated_jobs() {
        let mut selection = example();
        selection.set_visible("", false);
        selection.toggle_folder("v12/archive", "missing");
        assert!(selection.rows("missing", true).is_empty());
        assert!(selected_jobs(&selection).is_empty());
        selection.toggle_folder("v12/archive", "archive");
        assert_eq!(
            selected_jobs(&selection),
            ["v12/archive/sub/part10.zip", "v12/archive/part2.zip"]
        );
        selection.open_folder("v12/archive".into());
        selection.toggle_folder("v12/archive-extra", "");
        selection.toggle_folder("v12", "");
        assert_eq!(selected_jobs(&selection).len(), 2);
    }

    #[test]
    fn navigation_and_filters_do_not_change_the_global_download_jobs() {
        let mut selection = example();
        let initial = selected_jobs(&selection);
        assert_eq!(initial.len(), 6);
        assert_eq!(selection.rows("part10", true).len(), 1);
        selection.open_folder("v12/archive".into());
        selection.open_folder("v12/archive/sub".into());
        assert!(selection.rows("missing", false).is_empty());
        assert_eq!(selected_jobs(&selection), initial);
        selection.go_up();
        selection.go_up();
        selection.go_up();
        assert_eq!(selection.folder, selection.root);
        assert_eq!(selected_jobs(&selection), initial);
    }

    #[test]
    fn global_review_clears_and_restores_filter_without_losing_folder() {
        let mut selection = example();
        selection.set_visible("", false);
        selection.toggle_file(0);
        selection.toggle_file(3);
        selection.toggle_file(4);
        selection.open_folder("v12/archive".into());
        let review_filter = selection.toggle_selected_only("  PART10  ");
        assert!(review_filter.is_empty());
        assert_eq!(selection.rows(&review_filter, true).len(), 3);
        selection.go_up();
        selection.open_folder("v12/archive/sub".into());
        selection.toggle_folder("v12/archive/sub", "");
        assert_eq!(selection.folder, "v12/archive");
        assert_eq!(selected_jobs(&selection).len(), 3);
        selection.set_visible("addon", false);
        selection.set_visible("addon", true); // Cannot re-add hidden unchecked files.
        assert_eq!(
            selected_jobs(&selection),
            ["v12/archive/sub/part10.zip", "v12/alpha.txt"]
        );
        assert_eq!(selection.toggle_selected_only("addon"), "  PART10  ");
        assert_eq!(selection.folder, "v12/archive");
        assert_eq!(selection.rows("part10", true)[0].name, "sub");
        assert_eq!(selected_jobs(&selection).len(), 2);
    }

    #[test]
    fn duplicate_names_in_review_keep_distinct_job_ids_and_empty_selection() {
        let mut selection = example();
        selection.set_visible("", false);
        selection.set_visible("part2", true);
        selection.toggle_selected_only("");
        let rows = selection.rows("", true);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, rows[1].name);
        assert_ne!(rows[0].file_index, rows[1].file_index);
        let index = rows
            .iter()
            .find(|row| row.path == "v12/archive-extra/part2.zip")
            .unwrap()
            .file_index
            .unwrap();
        selection.toggle_file(index);
        assert_eq!(selected_jobs(&selection), ["v12/archive/part2.zip"]);
        selection.set_visible("", false);
        assert!(selection.rows("", true).is_empty());
        assert!(selected_jobs(&selection).is_empty());
    }
}
