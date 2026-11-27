//! Original ignore-aware indexing and tree projection, evaluated away from GPUI.
use crate::{
    background::LatestWorker,
    config::NavigationConfig,
    navigation::{self, ProjectIndex, TreeJob, TreeRow},
};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};
pub struct Explorer {
    pub root: PathBuf,
    pub visible: bool,
    pub focused: bool,
    pub selected: usize,
    pub width: f32,
    pub rows: Vec<TreeRow>,
    pub expanded: BTreeSet<PathBuf>,
    pub index: Option<Arc<ProjectIndex>>,
    indexer: Option<LatestWorker<(PathBuf, NavigationConfig), Arc<ProjectIndex>>>,
    projector: Option<LatestWorker<TreeJob, Vec<TreeRow>>>,
}
impl Explorer {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            visible: false,
            focused: false,
            selected: 0,
            width: 250.,
            rows: Vec::new(),
            expanded: BTreeSet::new(),
            index: None,
            indexer: None,
            projector: None,
        }
    }
    pub fn open(&mut self, root: PathBuf, config: NavigationConfig) {
        self.root = root.clone();
        self.index = None;
        self.rows.clear();
        self.expanded.clear();
        self.selected = 0;
        self.visible = true;
        if let Some(w) = &self.projector {
            w.cancel();
        }
        self.indexer
            .get_or_insert_with(|| {
                LatestWorker::new(|(root, config), cancel| {
                    Arc::new(navigation::build_index(root, &config, || {
                        cancel.cancelled()
                    }))
                })
            })
            .submit((root, config));
    }
    pub fn refresh(&mut self, config: NavigationConfig) {
        if let Some(indexer) = &self.indexer {
            indexer.submit((self.root.clone(), config));
        }
    }
    fn project(&mut self) {
        if let Some(index) = &self.index {
            self.projector
                .get_or_insert_with(|| LatestWorker::new(navigation::tree))
                .submit(TreeJob {
                    index: index.clone(),
                    expanded: self.expanded.clone(),
                });
        }
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(index) = self.indexer.as_ref().and_then(|w| w.poll())
            && index.root == self.root
        {
            self.index = Some(index);
            self.project();
            changed = true;
        }
        if let Some(rows) = self.projector.as_ref().and_then(|w| w.poll()) {
            self.rows = rows;
            self.selected = self.selected.min(self.rows.len().saturating_sub(1));
            changed = true;
        }
        changed
    }
    pub fn activate(&mut self, i: usize) -> Option<PathBuf> {
        self.selected = i;
        let row = self.rows.get(i)?;
        if row.entry.directory {
            let path = row.entry.relative.clone();
            if !self.expanded.remove(&path) {
                self.expanded.insert(path);
            }
            self.project();
            None
        } else {
            Some(row.entry.path.clone())
        }
    }
}
