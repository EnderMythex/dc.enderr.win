//! What the GitHub feeds and the status panel have already seen, kept in
//! `data/watch.json` so nothing is announced twice after a restart.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Mutex,
};

use dc_core::cards::Beat;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Highest release id seen, per repository.
    pub releases: HashMap<String, u64>,
    /// Last commit seen on the default branch, per repository.
    pub commits: HashMap<String, String>,
    /// Known state of pull requests and issues (`pr-12` -> `open`).
    pub tickets: HashMap<String, HashMap<String, String>>,
    /// Highest fork id seen.
    pub forks: HashMap<String, u64>,
    /// Known tags and branches (`tag:v1.0`, `branch:main`).
    pub refs: HashMap<String, Vec<String>>,
    pub stars: HashMap<String, u64>,
    /// Recent checks of each watched website, by URL, older first.
    pub history: HashMap<String, VecDeque<Beat>>,
}

pub struct Store {
    path: PathBuf,
    state: Mutex<State>,
}

impl Store {
    pub fn open(bot_dir: &std::path::Path) -> Self {
        let path = bot_dir.join("data").join("watch.json");
        let state = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    pub fn read<T>(&self, f: impl FnOnce(&State) -> T) -> T {
        f(&self.state.lock().unwrap())
    }

    /// Changes the state, then writes it (temporary file + rename).
    pub fn update<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.state.lock().unwrap();
        let out = f(&mut state);
        if let Ok(json) = serde_json::to_vec(&*state) {
            let tmp = self.path.with_extension("json.tmp");
            let written = self
                .path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&tmp, json))
                .and_then(|()| std::fs::rename(&tmp, &self.path));
            if let Err(error) = written {
                tracing::warn!(%error, "saving watch.json failed");
            }
        }
        out
    }
}
