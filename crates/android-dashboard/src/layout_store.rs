use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_tiles::Tree;
use serde::{Deserialize, Serialize};

use crate::layout::{PanelId, create_logs_tree, create_overview_tree};
use crate::tabs::MainTab;

const LAYOUT_VERSION: u32 = 2;
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Serialize, Deserialize)]
struct LayoutFile {
    version: u32,
    overview: Tree<PanelId>,
    logs: Tree<PanelId>,
}

/// Pretty-prints both tab tile trees as versioned JSON.
pub(crate) fn encode(
    overview: &Tree<PanelId>,
    logs: &Tree<PanelId>,
) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&LayoutFile {
        version: LAYOUT_VERSION,
        overview: overview.clone(),
        logs: logs.clone(),
    })
}

/// Parses versioned layout JSON into the two tab tile trees.
/// Returns None when the JSON is invalid, the version is not 2, or the
/// combined pane set is incomplete.
pub(crate) fn decode(json: &str) -> Option<(Tree<PanelId>, Tree<PanelId>)> {
    let file: LayoutFile = serde_json::from_str(json).ok()?;
    (file.version == LAYOUT_VERSION && panes_are_complete(&file.overview, &file.logs))
        .then_some((file.overview, file.logs))
}

/// Returns true when both trees have roots and each PanelId appears
/// exactly once across the two trees combined.
fn panes_are_complete(overview: &Tree<PanelId>, logs: &Tree<PanelId>) -> bool {
    let mut seen = HashSet::new();
    for tree in [overview, logs] {
        let Some(root) = tree.root else {
            return false;
        };
        if tree.tiles.get(root).is_none() {
            return false;
        }
        for tile in tree.tiles.tiles() {
            if let egui_tiles::Tile::Pane(pane) = tile {
                if !seen.insert(*pane) {
                    return false;
                }
            }
        }
    }
    seen.len() == PanelId::ALL.len() && PanelId::ALL.iter().all(|pane| seen.contains(pane))
}

fn default_trees() -> (Tree<PanelId>, Tree<PanelId>) {
    (create_overview_tree(), create_logs_tree())
}

/// Returns the on-disk path of the layout JSON, if a config directory exists.
pub(crate) fn layout_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("android-dashboard")
            .join("layout.json"),
    )
}

/// Reads layout JSON from `path`. Missing or invalid files yield the default trees.
pub(crate) fn load_from_path(path: &Path) -> (Tree<PanelId>, Tree<PanelId>) {
    match fs::read_to_string(path) {
        Err(err) if err.kind() == ErrorKind::NotFound => default_trees(),
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "failed to read layout"
            );
            default_trees()
        }
        Ok(json) => decode(&json).unwrap_or_else(|| {
            tracing::warn!(path = %path.display(), "invalid layout file");
            default_trees()
        }),
    }
}

/// Writes versioned layout JSON to `path` via a sibling `.tmp` file.
pub(crate) fn save_to_path(
    path: &Path,
    overview: &Tree<PanelId>,
    logs: &Tree<PanelId>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let json =
        encode(overview, logs).map_err(|err| std::io::Error::new(ErrorKind::InvalidData, err))?;
    let tmp = tmp_path(path);
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Loads the saved layout, or the default trees when none is available.
pub(crate) fn load_or_default() -> (Tree<PanelId>, Tree<PanelId>) {
    match layout_path() {
        Some(path) => load_from_path(&path),
        None => default_trees(),
    }
}

/// Writes the in-memory trees to the layout JSON path.
/// Returns true when the file was written.
pub(crate) fn save(overview: &Tree<PanelId>, logs: &Tree<PanelId>) -> bool {
    let Some(path) = layout_path() else {
        tracing::warn!("no config directory; layout not saved");
        return false;
    };
    if let Err(err) = save_to_path(&path, overview, logs) {
        tracing::warn!(
            error = %err,
            path = %path.display(),
            "failed to save layout"
        );
        return false;
    }
    true
}

#[derive(Serialize, Deserialize)]
struct UiStateFile {
    tab: String,
}

fn ui_state_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("android-dashboard")
            .join("ui-state.json"),
    )
}

/// Loads the persisted selected main tab; defaults to Overview.
pub(crate) fn load_selected_tab() -> MainTab {
    let Some(path) = ui_state_path() else {
        return MainTab::Overview;
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return MainTab::Overview;
    };
    let Ok(file) = serde_json::from_str::<UiStateFile>(&raw) else {
        return MainTab::Overview;
    };
    MainTab::from_ident(&file.tab).unwrap_or(MainTab::Overview)
}

/// Persists the selected main tab next to the layout file.
pub(crate) fn save_selected_tab(tab: MainTab) {
    let Some(path) = ui_state_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let file = UiStateFile {
        tab: tab.ident().to_string(),
    };
    if let Ok(json) = serde_json::to_string(&file) {
        let _ = fs::write(path, json);
    }
}

/// Debounces layout writes until edits have been idle.
pub(crate) struct LayoutSaver {
    dirty: bool,
    last_edit: Instant,
}

impl Default for LayoutSaver {
    fn default() -> Self {
        Self {
            dirty: false,
            last_edit: Instant::now(),
        }
    }
}

impl LayoutSaver {
    /// Records that the tile tree changed. Restarts the save delay.
    pub(crate) fn mark_edit(&mut self) {
        self.dirty = true;
        self.last_edit = Instant::now();
    }

    /// Writes the trees once the save delay has elapsed since the last edit.
    pub(crate) fn tick(
        &mut self,
        ctx: &egui::Context,
        overview: &Tree<PanelId>,
        logs: &Tree<PanelId>,
    ) {
        if !self.dirty {
            return;
        }
        let wait = SAVE_DEBOUNCE.saturating_sub(self.last_edit.elapsed());
        if wait.is_zero() {
            if save(overview, logs) {
                self.dirty = false;
            } else {
                ctx.request_repaint_after(SAVE_DEBOUNCE);
            }
        } else {
            ctx.request_repaint_after(wait);
        }
    }

    /// Writes a pending layout immediately. Clears dirty even if the write fails.
    pub(crate) fn flush(&mut self, overview: &Tree<PanelId>, logs: &Tree<PanelId>) {
        if !self.dirty {
            return;
        }
        save(overview, logs);
        self.dirty = false;
    }

    /// Drops a pending write without saving.
    #[allow(dead_code)]
    pub(crate) fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_layout_path() -> PathBuf {
        let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "android-dashboard-layout-test-{}-{n}.json",
            std::process::id()
        ))
    }

    struct TempLayout(PathBuf);

    impl TempLayout {
        fn new() -> Self {
            Self(temp_layout_path())
        }
    }

    impl Drop for TempLayout {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            let _ = fs::remove_file(tmp_path(&self.0));
        }
    }

    fn logs_tree_without_logcat_all() -> Tree<PanelId> {
        let mut tree = create_logs_tree();
        let ids: Vec<_> = tree
            .tiles
            .iter()
            .filter_map(|(id, tile)| match tile {
                egui_tiles::Tile::Pane(PanelId::LogcatAll) => Some(*id),
                _ => None,
            })
            .collect();
        for id in ids {
            tree.tiles.remove(id);
        }
        tree
    }

    #[test]
    fn encode_decode_default_trees_round_trip() {
        let (overview, logs) = default_trees();
        let json = encode(&overview, &logs).expect("encode");
        let restored = decode(&json).expect("decode");
        assert_eq!((overview, logs), restored);
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode("not json").is_none());
    }

    #[test]
    fn decode_rejects_other_version() {
        let (overview, logs) = default_trees();
        let mut value: serde_json::Value =
            serde_json::from_str(&encode(&overview, &logs).expect("encode")).expect("json");
        value["version"] = serde_json::json!(3);
        assert!(decode(&value.to_string()).is_none());
    }

    #[test]
    fn decode_rejects_missing_pane() {
        let (overview, _) = default_trees();
        let json = encode(&overview, &logs_tree_without_logcat_all()).expect("encode");
        assert!(decode(&json).is_none());
    }

    #[test]
    fn load_missing_path_yields_default_trees() {
        let temp = TempLayout::new();
        assert!(!temp.0.exists());
        assert_eq!(load_from_path(&temp.0), default_trees());
    }

    #[test]
    fn save_then_load_round_trips() {
        let temp = TempLayout::new();
        let (overview, logs) = default_trees();
        save_to_path(&temp.0, &overview, &logs).expect("save");
        assert_eq!(load_from_path(&temp.0), (overview, logs));
    }
}
