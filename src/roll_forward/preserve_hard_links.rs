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
// Copyright (c) 2023, Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use crate::data::paths::BasicDirEntryInfo;
use crate::library::file_ops::Copy;
use crate::library::file_ops::Preserve;
use crate::library::file_ops::Remove;
use crate::library::results::{HttmError, HttmResult};
use crate::RollForward;
use hashbrown::{HashMap, HashSet};
use nu_ansi_term::Color::{Green, Yellow};
use rayon::prelude::*;
use std::fs::read_dir;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

// key: inode, values: Paths
pub struct HardLinkMap {
    link_map: HashMap<u64, Vec<BasicDirEntryInfo>>,
    remainder: HashSet<PathBuf>,
}

impl HardLinkMap {
    pub fn new(requested_path: &Path) -> HttmResult<Self> {
        let constructed = BasicDirEntryInfo {
            path: requested_path.to_path_buf(),
            file_type: None,
        };

        let mut queue: Vec<BasicDirEntryInfo> = vec![constructed];
        let mut tmp: HashMap<u64, Vec<BasicDirEntryInfo>> = HashMap::new();

        // condition kills iter when user has made a selection
        // pop_back makes this a LIFO queue which is supposedly better for caches
        while let Some(item) = queue.pop() {
            // no errors will be propagated in recursive mode
            // far too likely to run into a dir we don't have permissions to view
            let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
                read_dir(item.path)?
                    .flatten()
                    // checking file_type on dir entries is always preferable
                    // as it is much faster than a metadata call on the path
                    .map(|dir_entry| BasicDirEntryInfo::from(&dir_entry))
                    .partition(|dir_entry| dir_entry.path.is_dir());

            let mut combined = vec_files;
            combined.extend_from_slice(&vec_dirs);
            queue.extend_from_slice(&vec_dirs);

            combined
                .into_iter()
                .filter(|entry| {
                    if let Some(ft) = entry.file_type {
                        return ft.is_file();
                    }

                    false
                })
                .filter_map(|entry| entry.path.metadata().ok().map(|md| (md.ino(), entry)))
                .for_each(|(ino, entry)| match tmp.get_mut(&ino) {
                    Some(values) => values.push(entry),
                    None => {
                        let _ = tmp.insert(ino, vec![entry]);
                    }
                });
        }

        let (link_map, remain_tmp): (
            HashMap<u64, Vec<BasicDirEntryInfo>>,
            HashMap<u64, Vec<BasicDirEntryInfo>>,
        ) = tmp.into_iter().partition(|(_ino, values)| values.len() > 1);

        let remainder = remain_tmp
            .into_values()
            .flatten()
            .map(|entry| entry.path)
            .collect();

        Ok(Self {
            link_map,
            remainder,
        })
    }
}

pub struct PreserveHardLinks<'a> {
    live_map: &'a HardLinkMap,
    snap_map: &'a HardLinkMap,
    roll_forward: &'a RollForward,
}

impl<'a> PreserveHardLinks<'a> {
    pub fn new(
        live_map: &'a HardLinkMap,
        snap_map: &'a HardLinkMap,
        roll_forward: &'a RollForward,
    ) -> HttmResult<Self> {
        Ok(Self {
            live_map,
            snap_map,
            roll_forward,
        })
    }

    pub fn exec(&self) -> HttmResult<HashSet<PathBuf>> {
        eprintln!("Removing and preserving the difference between live and snap orphans.");
        let mut exclusions = self.diff_orphans()?;

        eprintln!(
      "Removing the intersection of the live and snap hard link maps to generate snap orphans."
    );
        let intersection = self.remove_map_intersection()?;
        exclusions.extend(intersection);

        eprintln!("Removing additional unnecessary links on the live dataset.");
        self.remove_live_links()?;
        exclusions.extend(
            self.live_map
                .link_map
                .clone()
                .into_values()
                .flatten()
                .map(|entry| entry.path),
        );

        eprintln!("Preserving necessary links from the snapshot dataset.");
        self.preserve_snap_links()?;
        exclusions.extend(
            self.snap_map
                .link_map
                .clone()
                .into_values()
                .flatten()
                .map(|entry| entry.path),
        );

        Ok(exclusions)
    }

    fn remove_live_links(&self) -> HttmResult<()> {
        let none_removed = AtomicBool::new(true);

        self.live_map
            .link_map
            .par_iter()
            .try_for_each(|(_key, values)| {
                values.iter().try_for_each(|live_path| {
                    let snap_path = self
                        .roll_forward
                        .snap_path(&live_path.path)
                        .ok_or_else(|| HttmError::new("Could obtain live path for snap path"))?;

                    if !snap_path.exists() {
                        none_removed.store(false, std::sync::atomic::Ordering::Relaxed);
                        return Self::rm_hard_link(&live_path.path);
                    }

                    Ok(())
                })
            })?;

        if none_removed.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("No hard links found which require removal.");
            return Ok(());
        }

        Ok(())
    }

    fn preserve_snap_links(&self) -> HttmResult<()> {
        let none_preserved = AtomicBool::new(true);

        self.snap_map
            .link_map
            .par_iter()
            .try_for_each(|(_key, values)| {
                let complemented_paths: Vec<(PathBuf, &PathBuf)> = values
                    .iter()
                    .map(|snap_path| {
                        let live_path =
                            self.roll_forward.live_path(&snap_path.path).ok_or_else(|| {
                                HttmError::new("Could obtain live path for snap path").into()
                            });

                        live_path.map(|live| (live, &snap_path.path))
                    })
                    .collect::<HttmResult<Vec<(PathBuf, &PathBuf)>>>()?;

                let mut opt_original = complemented_paths
                    .iter()
                    .map(|(live, _snap)| live)
                    .find(|path| path.exists());

                complemented_paths
                    .iter()
                    .filter(|(_live_path, snap_path)| snap_path.exists())
                    .try_for_each(|(live_path, snap_path)| {
                        none_preserved.store(false, std::sync::atomic::Ordering::Relaxed);

                        match opt_original {
                            Some(original) if original == live_path => {
                                RollForward::copy(snap_path, live_path)
                            }
                            Some(original) => self.hard_link(original, live_path),
                            None => {
                                opt_original = Some(live_path);
                                RollForward::copy(snap_path, live_path)
                            }
                        }
                    })
            })?;

        if none_preserved.load(std::sync::atomic::Ordering::Relaxed) {
            println!("No hard links found which require preservation.");
            return Ok(());
        }

        Ok(())
    }

    fn snaps_to_live_remainder(&self) -> HttmResult<HashSet<PathBuf>> {
        // in self but not in other
        self.snap_map
            .remainder
            .par_iter()
            .map(|snap_path| {
                self.roll_forward
                    .live_path(snap_path)
                    .ok_or_else(|| HttmError::new("Could obtain live path for snap path").into())
            })
            .collect::<HttmResult<HashSet<PathBuf>>>()
    }

    fn snaps_to_live_map(&self) -> HttmResult<HashSet<PathBuf>> {
        // in self but not in other
        self.snap_map
            .link_map
            .par_iter()
            .flat_map(|(_key, values)| values)
            .map(|snap_entry| {
                self.roll_forward
                    .live_path(&snap_entry.path)
                    .ok_or_else(|| HttmError::new("Could obtain live path for snap path").into())
            })
            .collect::<HttmResult<HashSet<PathBuf>>>()
    }

    fn diff_orphans(&'a self) -> HttmResult<HashSet<PathBuf>> {
        let snaps_to_live_remainder = self.snaps_to_live_remainder()?;
        let live_diff = self.live_map.remainder.difference(&snaps_to_live_remainder);
        let snap_diff = snaps_to_live_remainder.difference(&self.live_map.remainder);

        // only on live dataset - means we want to delete these
        live_diff
            .clone()
            .par_bridge()
            .try_for_each(|path| RollForward::remove(path))?;

        // only on snap dataset - means we want to copy these
        snap_diff.clone().par_bridge().try_for_each(|live_path| {
            let snap_path: HttmResult<PathBuf> =
                RollForward::snap_path(self.roll_forward, live_path)
                    .ok_or_else(|| HttmError::new("Could obtain live path for snap path").into());

            RollForward::copy(&snap_path?, live_path)
        })?;

        let combined = live_diff.chain(snap_diff).cloned().collect();

        Ok(combined)
    }

    fn remove_map_intersection(&self) -> HttmResult<HashSet<PathBuf>> {
        let snaps_to_live_map = self.snaps_to_live_map()?;
        let live_map_as_set: HashSet<PathBuf> = self
            .live_map
            .link_map
            .clone()
            .into_values()
            .flatten()
            .map(|entry| entry.path)
            .collect();

        let orphans_intersection = live_map_as_set.intersection(&snaps_to_live_map);

        // this is repeating the step of orphaning a link
        // intersection is removed and recreated later, leaving dangling hard links
        orphans_intersection
            .clone()
            .par_bridge()
            .try_for_each(|live_path| Self::rm_hard_link(live_path))?;

        let res = orphans_intersection.cloned().collect();

        Ok(res)
    }

    fn hard_link(&self, original: &Path, link: &Path) -> HttmResult<()> {
        if !original.exists() {
            let msg = format!(
                "Cannot link because original path does not exists: {:?}",
                original
            );
            return Err(HttmError::new(&msg).into());
        }

        if link.exists() {
            if let Ok(og_md) = original.symlink_metadata() {
                if let Ok(link_md) = link.symlink_metadata() {
                    if og_md.ino() == link_md.ino() {
                        return Ok(());
                    }
                }
            }

            Remove::recursive_quiet(link)?
        }

        Copy::generate_dst_parent(link)?;

        if let Err(err) = std::fs::hard_link(original, link) {
            if !link.exists() {
                eprintln!("Error: {}", err);
                let msg = format!("Could not link file {:?} to {:?}", original, link);
                return Err(HttmError::new(&msg).into());
            }
        }

        if let Some(snap_path) = self.roll_forward.snap_path(link) {
            Preserve::recursive(&snap_path, link)?;
        } else {
            return Err(HttmError::new("Could not obtain snap path").into());
        }

        eprintln!("{}: {:?} -> {:?}", Yellow.paint("Linked  "), original, link);

        Ok(())
    }

    fn rm_hard_link(link: &Path) -> HttmResult<()> {
        match Remove::recursive_quiet(link) {
            Ok(_) => {
                if link.exists() {
                    let msg = format!("Target link should not exist after removal {:?}", link);
                    return Err(HttmError::new(&msg).into());
                }
            }
            Err(err) => {
                if link.exists() {
                    eprintln!("Error: {}", err);
                    let msg = format!("Could not remove link {:?}", link);
                    return Err(HttmError::new(&msg).into());
                }
            }
        }

        eprintln!("{}: {:?} -> 🗑️", Green.paint("Unlinked  "), link);

        Ok(())
    }
}
