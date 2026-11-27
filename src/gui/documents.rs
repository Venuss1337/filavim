//! Central owner of real per-document editors. Switching moves ownership; no text is duplicated.
//!
//! The active model is accessed through `Deref`; inactive models retain their
//! cursors and undo; switching clears outgoing composition and keymap prefixes.
//! Stable document IDs identify asynchronous work; tab order and path identity
//! are separate from the active slot.
use super::model::Model;
use crate::{
    buffer::Buffer,
    editor::{Editor, Mode, Request},
    input::Action,
    key::{KeyCode, KeyEvent},
};
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    path::Path,
};
/// Owns the active model and inactive tab models; never an independent text copy.
pub struct Documents {
    active: Model,
    inactive: BTreeMap<u64, Model>,
    pub order: Vec<u64>,
    serial: u64,
    pub action: Option<Action>,
    pub command: Option<String>,
}
impl Deref for Documents {
    type Target = Model;
    fn deref(&self) -> &Model {
        &self.active
    }
}
impl DerefMut for Documents {
    fn deref_mut(&mut self) -> &mut Model {
        &mut self.active
    }
}
impl Documents {
    pub fn new(mut active: Model) -> Self {
        active.editor.document_id = 1;
        Self {
            active,
            inactive: BTreeMap::new(),
            order: vec![1],
            serial: 1,
            action: None,
            command: None,
        }
    }
    pub fn id(&self) -> u64 {
        self.active.editor.document_id
    }
    pub fn get(&self, id: u64) -> Option<&Editor> {
        if self.id() == id {
            Some(&self.active.editor)
        } else {
            self.inactive.get(&id).map(|m| &m.editor)
        }
    }
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Editor> {
        if self.id() == id {
            Some(&mut self.active.editor)
        } else {
            self.inactive.get_mut(&id).map(|m| &mut m.editor)
        }
    }
    pub fn switch(&mut self, id: u64) -> bool {
        if self.id() == id {
            return true;
        }
        let Some(next) = self.inactive.remove(&id) else {
            return false;
        };
        self.active.unmark();
        self.active.keymap.clear();
        let old = std::mem::replace(&mut self.active, next);
        self.inactive.insert(old.editor.document_id, old);
        true
    }
    pub fn cycle(&mut self, back: bool) {
        let i = self
            .order
            .iter()
            .position(|id| *id == self.id())
            .unwrap_or(0);
        let n = self.order.len();
        self.switch(self.order[(i + if back { n - 1 } else { 1 }) % n]);
    }
    pub fn find(&self, path: &Path) -> Option<u64> {
        let key = crate::lsp::path_key(path);
        self.order.iter().copied().find(|id| {
            self.get(*id)
                .and_then(|e| e.buffer.path.as_ref())
                .is_some_and(|p| crate::lsp::path_key(p) == key)
        })
    }
    pub fn add(&mut self, buffer: Buffer, activate: bool) -> anyhow::Result<u64> {
        if let Some(id) = buffer.path.as_ref().and_then(|p| self.find(p)) {
            if activate {
                self.switch(id);
            }
            return Ok(id);
        }
        let mut next = Model::new(self.active.editor.config.clone())?;
        next.set_buffer(buffer);
        self.serial += 1;
        next.editor.document_id = self.serial;
        let id = self.serial;
        if activate {
            let old = std::mem::replace(&mut self.active, next);
            if old.editor.buffer.path.is_none()
                && !old.editor.buffer.dirty()
                && old.editor.buffer.text.len_chars() == 0
            {
                self.order.retain(|i| *i != old.editor.document_id);
            } else {
                self.inactive.insert(old.editor.document_id, old);
            }
        } else {
            self.inactive.insert(id, next);
        }
        self.order.push(id);
        Ok(id)
    }
    /// Replacing a document retires its identity, even when the new revision is zero.
    /// Replace only the requested, unchanged document; newer edits reject the reload.
    pub fn reload(&mut self, id: u64, revision: u64, buffer: Buffer) -> anyhow::Result<bool> {
        if self.get(id).is_none_or(|e| e.buffer.revision != revision) {
            return Ok(false);
        }
        let active = self.id() == id;
        self.close_document(id, true).map_err(anyhow::Error::msg)?;
        self.add(buffer, active)?;
        Ok(true)
    }
    pub fn configure(&mut self, config: crate::config::Config) -> anyhow::Result<()> {
        crate::input::Keymap::new(&config)?;
        self.active.keymap = crate::input::Keymap::new(&config)?;
        self.active.editor.config = config.clone();
        for model in self.inactive.values_mut() {
            model.keymap = crate::input::Keymap::new(&config)?;
            model.editor.config = config.clone();
        }
        Ok(())
    }
    pub fn close_document(&mut self, id: u64, force: bool) -> Result<(), String> {
        let Some(e) = self.get(id) else {
            return Ok(());
        };
        if e.buffer.dirty() && !force {
            return Err("Unsaved changes".into());
        }
        if self.order.len() == 1 {
            let mut next = Model::new(self.editor.config.clone()).map_err(|e| e.to_string())?;
            self.serial += 1;
            next.editor.document_id = self.serial;
            self.active = next;
            self.order = vec![self.serial];
            return Ok(());
        }
        if id == self.id() {
            let at = self.order.iter().position(|i| *i == id).unwrap_or(0);
            let next = self.order[(at + 1) % self.order.len()];
            self.switch(next);
        }
        self.inactive.remove(&id);
        self.order.retain(|i| *i != id);
        Ok(())
    }
    pub fn dirty_ids(&self) -> Vec<u64> {
        self.order
            .iter()
            .copied()
            .filter(|id| self.get(*id).is_some_and(|e| e.buffer.dirty()))
            .collect()
    }
    pub fn key(&mut self, key: KeyEvent) -> (bool, Option<Request>) {
        if self.marked.is_some() && key.code != KeyCode::Esc {
            return (false, None);
        }
        let mode = self.editor.mode;
        let Some(action) = self.keymap.resolve(mode, key) else {
            return (false, None);
        };
        self.action = Some(action);
        if action == Action::Submit && mode == Mode::Command && !self.editor.searching {
            self.command = Some(std::mem::take(&mut self.editor.command));
            self.editor.set_mode(Mode::Normal);
            return (true, None);
        }
        if action == Action::New {
            let _ = self.add(Buffer::default(), true);
            return (true, None);
        }
        (true, self.active.act(action))
    }
}
