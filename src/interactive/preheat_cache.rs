//       ___           ___           ___           ___
//      /\__\         /\  \         /\  \         /\__\
//     /:/  /         \:\  \        \:\  \       /::|  |
//    /:/__/           \:\  \        \:\  \     /:|:|  |
//   /::\  \ ___       /::\  \       /::\  \   /:/|:|__|__
//  /:/\:\  /\__\     /:/\:\__\     /:/\:\__\ /:/ |::::\__\
//  \/__\:\/:/  /    /:/  \/__/    /:/  \/__/ \/__/~~/:/  /
//       \::/  /    /:/  /        /:/  /            /:/  /
//       /:/  /     \/__/         \/__/            /:/  /
//      /:/  /                                    /:/  /
//      \/__/                                     \/__/
//
// Copyright (c) 2025, Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use crate::data::paths::{
    PathData,
    PathDeconstruction,
};
use crate::lookup::versions::RelativePathAndSnapMounts;
use hashbrown::HashSet;
use std::path::{
    Path,
    PathBuf,
};
use std::sync::atomic::AtomicBool;
use std::sync::{
    Arc,
    RwLock,
    TryLockError,
};
use std::time::Duration;

#[derive(Debug)]
pub struct PreheatCache {
    set: RwLock<HashSet<PathBuf>>,
    hangup: AtomicBool,
}

impl PreheatCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            set: RwLock::new(HashSet::new()),
            hangup: AtomicBool::new(false),
        })
    }

    #[allow(dead_code)]
    pub fn clear(self: &Arc<Self>) {
        self.hangup
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let inner = self.clone();

        rayon::spawn(move || {
            match inner.set.write() {
                Ok(mut map) => map.clear(),
                Err(_err) => {
                    inner.set.clear_poison();
                }
            };
        });
    }

    #[inline(always)]
    pub fn exec(self: &Arc<Self>, bundle: &RelativePathAndSnapMounts) {
        if self
            .set
            .try_read()
            .ok()
            .map(|cached_result| cached_result.contains(bundle.dataset_of_interest()))
            .unwrap_or_else(|| true)
        {
            return;
        }

        if self.hangup.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }

        let inner_clone = self.clone();
        let path_data_clone = bundle.path_data().clone();
        let dataset_of_interest_clone = bundle.dataset_of_interest().to_path_buf();
        let config_clone = bundle.config().clone();

        rayon::spawn_fifo(move || {
            let mut backoff: u64 = 2;

            let vec: Vec<PathBuf> = loop {
                if inner_clone
                    .hangup
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    return;
                }

                match inner_clone.set.try_write() {
                    Ok(mut locked) => {
                        break Self::proximate_plus_parent_and_top_level(
                            &path_data_clone,
                            &dataset_of_interest_clone,
                        )
                        .into_iter()
                        .filter(|item| locked.insert(item.to_path_buf()))
                        .collect();
                    }
                    Err(err) => {
                        if let TryLockError::Poisoned(_) = err {
                            inner_clone.set.clear_poison();
                        }
                        std::thread::sleep(Duration::from_millis(backoff));
                        backoff *= 2;
                        continue;
                    }
                }
            };

            vec.iter()
                .filter_map(|dataset| {
                    RelativePathAndSnapMounts::snap_mounts_from_dataset_of_interest(
                        &dataset,
                        &config_clone,
                    )
                })
                .take_while(|_bundle| {
                    !inner_clone
                        .hangup
                        .load(std::sync::atomic::Ordering::Relaxed)
                })
                .for_each(|bundle| {
                    let _ = bundle
                        .into_iter()
                        .map(|snap_path| std::fs::read_dir(snap_path))
                        .flatten()
                        .flatten()
                        .flatten()
                        .next();
                });
        });
    }

    #[inline(always)]
    pub fn proximate_plus_parent_and_top_level(
        pathdata: &PathData,
        proximate_dataset: &Path,
    ) -> Vec<PathBuf> {
        // for /usr/bin, we prefer the most proximate: /usr/bin to /usr and /
        // ancestors() iterates in this top-down order, when a value: dataset/fstype is available
        // we map to return the key, instead of the value
        let mut res = vec![proximate_dataset.to_path_buf()];

        match pathdata
            .path()
            .parent()
            .map(|path| PathData::without_styling(path, None))
            .map(|pathdata| {
                pathdata
                    .proximate_dataset()
                    .ok()
                    .map(|path| path.to_owned())
            })
            .flatten()
            .filter(|parent_dataset| parent_dataset != proximate_dataset)
        {
            Some(parent_dataset) => {
                res.push(parent_dataset);
            }
            _ => (),
        };

        let dir_iter = std::fs::read_dir(pathdata.path())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|de| match de.file_type().ok() {
                Some(ft) if ft.is_dir() => {
                    Some(PathData::without_styling(&de.path(), de.metadata().ok()))
                }
                Some(ft) if ft.is_symlink() => std::fs::read_link(de.path())
                    .ok()
                    .map(|path| PathData::without_styling(&path, None)),
                _ => None,
            })
            .filter_map(|pd| pd.proximate_dataset().ok().map(|path| path.to_owned()))
            .filter(|parent_dataset| parent_dataset != proximate_dataset);

        res.extend(dir_iter);

        res.sort();
        res.dedup();

        res
    }
}
