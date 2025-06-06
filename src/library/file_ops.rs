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

use crate::data::paths::{PathData, PathDeconstruction};
use crate::library::diff_copy::HttmCopy;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::is_metadata_same;
use crate::{GLOBAL_CONFIG, IN_BUFFER_SIZE};
use nix::sys::stat::SFlag;
use nu_ansi_term::Color::{Red, Yellow};
use std::fs::{create_dir_all, read_dir, set_permissions};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, ErrorKind};
use std::iter::Iterator;
use std::os::unix::fs::{FileTypeExt, MetadataExt, chown};
use std::path::Path;

const CHAR_KIND: SFlag = nix::sys::stat::SFlag::S_IFCHR;
const BLK_KIND: SFlag = nix::sys::stat::SFlag::S_IFBLK;

pub struct Copy;

impl Copy {
    pub fn recursive_quiet(src: &Path, dst: &Path, should_preserve: bool) -> HttmResult<()> {
        if src.is_dir() {
            Self::direct_quiet(src, dst, should_preserve)?;

            for entry in read_dir(&src)?.flatten() {
                let file_type = entry.file_type()?;
                let entry_src = entry.path();
                let entry_dst = dst.join(entry.file_name());

                if entry_src.exists() {
                    if file_type.is_dir() {
                        Self::recursive_quiet(&entry_src, &entry_dst, should_preserve)?;
                    } else {
                        Self::direct_quiet(&entry_src, &entry_dst, should_preserve)?;
                    }
                }
            }
        } else {
            Self::direct_quiet(&src, dst, should_preserve)?;
        }

        if should_preserve {
            // macos likes to fail on the metadata copy
            match Preserve::recursive(src, dst) {
                Ok(_) => {}
                Err(err) => {
                    if is_metadata_same(src, dst).is_ok() {
                        if GLOBAL_CONFIG.opt_debug {
                            eprintln!(
                                "WARN: The OS reports an error that it was unable to copy file metadata for the following reason: {}",
                                err.to_string().trim_end()
                            );
                            eprintln!(
                                "NOTICE: This is most likely because such feature is unsupported by this OS.  httm confirms basic file metadata (size and mtime) are the same for transfer: {:?} -> {:?}.",
                                src, dst
                            )
                        }
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        Ok(())
    }

    pub fn generate_dst_parent(dst: &Path) -> HttmResult<()> {
        if let Some(dst_parent) = dst.parent() {
            create_dir_all(dst_parent)?;
            Ok(())
        } else {
            let description = format!("Could not detect a parent for destination file: {:?}", dst);
            HttmError::from(description).into()
        }
    }

    pub fn direct_quiet(src: &Path, dst: &Path, should_preserve: bool) -> HttmResult<()> {
        if src.is_dir() {
            create_dir_all(&dst)?;
        } else {
            Self::generate_dst_parent(&dst)?;

            if src.is_file() {
                HttmCopy::new(&src, &dst)?;
            } else {
                if dst.exists() {
                    Remove::recursive_quiet(dst)?;
                }
                if src.is_symlink() {
                    let link_target = std::fs::read_link(&src)?;
                    std::os::unix::fs::symlink(&link_target, &dst)?;
                } else {
                    Self::special_file(src, dst)?;
                }
            }
        }

        if should_preserve {
            Preserve::direct(src, dst)?
        }

        Ok(())
    }

    fn special_file(src: &Path, dst: &Path) -> HttmResult<()> {
        let src_metadata = src.symlink_metadata()?;
        let src_file_type = src_metadata.file_type();
        let src_mode_bits = src_metadata.mode();
        #[cfg(target_os = "linux")]
        let dst_mode = nix::sys::stat::Mode::from_bits_truncate(src_mode_bits);
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        let dst_mode = nix::sys::stat::Mode::from_bits_truncate(src_mode_bits as u16);

        let is_blk = src_file_type.is_block_device();
        let is_char = src_file_type.is_char_device();
        let is_fifo = src_file_type.is_fifo();
        let is_socket = src_file_type.is_socket();

        if is_blk || is_char {
            let dev = src_metadata.dev();
            let kind = if is_blk { BLK_KIND } else { CHAR_KIND };
            #[cfg(target_os = "linux")]
            nix::sys::stat::mknod(dst, kind, dst_mode, dev)?;
            #[cfg(target_os = "macos")]
            nix::sys::stat::mknod(dst, kind, dst_mode, dev as i32)?;
            #[cfg(target_os = "freebsd")]
            nix::sys::stat::mknod(dst, kind, dst_mode, dev as u32)?;
        } else if is_fifo {
            // create new fifo
            nix::unistd::mkfifo(dst, dst_mode)?;
        } else if is_socket {
            let description = format!(
                "WARN: Source path could not be copied.  Source path is a socket, and sockets are not considered within the scope of httm.  \
            Traditionally, sockets could not be copied, and they should always be recreated by the generating daemon, when deleted: \"{}\"",
                src.display()
            );
            eprintln!("{}", description)
        } else {
            let description = format!(
                "httm could not determine the source path's file type, and therefore it could not be copied.  \
            The source path was not recognized as a directory, regular file, device, fifo, socket, or symlink.  \
            Other special file types (like doors and event ports) are unsupported: \"{}\"",
                src.display()
            );
            return HttmError::from(description).into();
        }

        Ok(())
    }

    pub fn atomic_swap(
        src: &Path,
        dst: &Path,
        dst_tmp_path: &Path,
        should_preserve: bool,
    ) -> HttmResult<()> {
        fn swap(
            src: &Path,
            dst: &Path,
            dst_tmp_path: &Path,
            should_preserve: bool,
        ) -> HttmResult<()> {
            Copy::recursive_quiet(src, dst_tmp_path, should_preserve)?;
            Remove::recursive_quiet(dst)?;
            Rename::direct_quiet(dst_tmp_path, &dst)?;

            Ok(())
        }

        if let Err(err) = swap(src, dst, dst_tmp_path, should_preserve) {
            if dst_tmp_path.exists() {
                let _ = Remove::recursive_quiet(&dst_tmp_path);
            }

            return Err(err.into());
        }

        Ok(())
    }
}

pub struct Rename;

impl Rename {
    #[allow(dead_code)]
    pub fn direct(src: &Path, dst: &Path) -> HttmResult<()> {
        Self::direct_quiet(src, dst)?;

        eprintln!("{}: {:?} -> {:?}", Yellow.paint("Renamed "), src, dst);

        Ok(())
    }

    pub fn direct_quiet(src: &Path, dst: &Path) -> HttmResult<()> {
        std::fs::rename(src, dst)?;

        Ok(())
    }
}

pub struct Preserve;

impl Preserve {
    pub fn direct(src: &Path, dst: &Path) -> HttmResult<()> {
        let src_metadata = src.symlink_metadata()?;
        let dst_file = std::fs::File::options()
            .create(false)
            .read(true)
            .write(false)
            .open(&dst)?;

        // Mode
        {
            set_permissions(dst, src_metadata.permissions())?
        }

        // ACLs - requires libacl1-dev to build
        #[cfg(feature = "acls")]
        {
            if let Ok(acls) = exacl::getfacl(src, None) {
                exacl::setfacl(&[dst], &acls, None)?;
            }
        }

        // Ownership
        {
            let dst_uid = src_metadata.uid();
            let dst_gid = src_metadata.gid();

            chown(dst, Some(dst_uid), Some(dst_gid))?
        }

        // XAttrs
        {
            #[cfg(feature = "xattrs")]
            if let Ok(xattrs) = xattr::list(src) {
                xattrs
                    .flat_map(|attr| {
                        xattr::get(src, attr.clone()).map(|opt_value| (attr, opt_value))
                    })
                    .filter_map(|(attr, opt_value)| opt_value.map(|value| (attr, value)))
                    .try_for_each(|(attr, value)| xattr::set(dst, attr, value.as_slice()))?
            }
        }

        // Timestamps
        {
            let src_times = std::fs::FileTimes::new()
                .set_accessed(src_metadata.accessed()?)
                .set_modified(src_metadata.modified()?);

            dst_file.set_times(src_times)?;
        }

        dst_file.sync_all()?;

        Ok(())
    }

    pub fn recursive(src: &Path, dst: &Path) -> HttmResult<()> {
        let dst_path_data: PathData = dst.into();

        let proximate_dataset_mount = dst_path_data.proximate_dataset()?;

        let Ok(relative_path) = dst_path_data.relative_path(proximate_dataset_mount) else {
            let description = format!(
                "Could not determine relative path for destination: {:?}",
                dst
            );
            return HttmError::from(description).into();
        };

        let relative_path_components_len = relative_path.components().count();

        src.ancestors()
            .zip(dst.ancestors())
            .take(relative_path_components_len)
            .try_for_each(|(src_ancestor, dst_ancestor)| {
                Preserve::direct(src_ancestor, dst_ancestor)
            })
    }
}

pub struct Remove;

impl Remove {
    pub fn recursive(src: &Path) -> HttmResult<()> {
        Self::recursive_quiet(src)?;

        eprintln!("{}: {:?} -> 🗑️", Red.paint("Removed  "), src);

        Ok(())
    }

    pub fn recursive_quiet(src: &Path) -> HttmResult<()> {
        if src.is_dir() {
            for entry in read_dir(&src)?.flatten() {
                let file_type = entry.file_type()?;
                let path = entry.path();

                if path.exists() {
                    if file_type.is_dir() {
                        Self::recursive(&path)?;
                    } else {
                        std::fs::remove_file(path)?
                    }
                }
            }

            if src.exists() {
                std::fs::remove_dir_all(&src)?
            }
        } else if src.exists() {
            std::fs::remove_file(&src)?
        }

        Ok(())
    }
}

pub struct ChecksumFileContents<'a> {
    inner: &'a Path,
}

impl<'a> ChecksumFileContents<'a> {
    pub fn checksum(&self) -> u64 {
        use foldhash::quality::FixedState;
        use std::hash::{BuildHasher, Hasher};

        let s = FixedState::default();
        let mut hash = s.build_hasher();

        self.hash(&mut hash);

        hash.finish()
    }
}

impl<'a> From<&'a Path> for ChecksumFileContents<'a> {
    fn from(path: &'a Path) -> Self {
        Self { inner: path }
    }
}

impl<'a> Hash for ChecksumFileContents<'a> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let Some(self_file) = std::fs::OpenOptions::new()
            .read(true)
            .open(&self.inner)
            .ok()
        else {
            return;
        };

        let mut reader = BufReader::with_capacity(IN_BUFFER_SIZE, self_file);

        loop {
            let consumed = match reader.fill_buf() {
                Ok(buf) => {
                    if buf.is_empty() {
                        return;
                    }

                    state.write(buf);
                    buf.len()
                }
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted => continue,
                    ErrorKind::UnexpectedEof => {
                        return;
                    }
                    _ => return,
                },
            };

            reader.consume(consumed);
        }
    }
}

pub fn is_same_file_contents(first: &Path, second: &Path) -> bool {
    let (self_hash, other_hash): (u64, u64) = rayon::join(
        || ChecksumFileContents::from(first).checksum(),
        || ChecksumFileContents::from(second.as_ref()).checksum(),
    );

    self_hash == other_hash
}
