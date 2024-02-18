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

// this module is a re-implementation of the diff_copy() method, as used by the lms crate,
// which served as a basis as to how to implement.
//
// see original: https://github.com/wchang22/LuminS/blob/9efedd6f20c74aa75261e51ac1c95ee883f7e65b/src/lumins/file_ops.rs#L63
//
// though I am fairly certain this implementation is fair use, I've reproduced his license,
// as of 3/30/2023, verbatim below:

// Copyright (c) 2019 Wesley Chang

// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:

// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use crate::config::generate::ListSnapsOfType;
use crate::data::paths::{CompareVersionsContainer, PathData};
use crate::library::results::HttmError;
use crate::library::results::HttmResult;
use crate::GLOBAL_CONFIG;
use once_cell::sync::Lazy;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, ErrorKind, Seek, SeekFrom, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;
use std::process::Command as ExecProcess;
use std::sync::atomic::AtomicBool;

const CHUNK_SIZE: usize = 65_536;

static IS_CLONE_COMPATIBLE: Lazy<AtomicBool> = Lazy::new(|| {
    if let Ok(zfs_command) = which::which("zfs") {
        let Ok(process_output) = ExecProcess::new(zfs_command).arg("-V").output() else {
            return AtomicBool::new(false);
        };

        if !process_output.stderr.is_empty() {
            return AtomicBool::new(false);
        }

        let Ok(stdout) = std::str::from_utf8(&process_output.stdout) else {
            return AtomicBool::new(false);
        };

        if stdout.contains("zfs-2.2.0")
            || stdout.contains("zfs-kmod-2.2.0")
            || stdout.contains("zfs-2.2.1")
            || stdout.contains("zfs-kmod-2.2.1")
            || stdout.contains("zfs-2.2-")
            || stdout.contains("zfs-kmod-2.2-")
        {
            return AtomicBool::new(false);
        }
    }

    AtomicBool::new(true)
});

enum DstFileState {
    Exists,
    DoesNotExist,
}

impl DstFileState {
    fn exists(dst_file: &File) -> Self {
        if dst_file.metadata().is_ok() {
            DstFileState::Exists
        } else {
            DstFileState::DoesNotExist
        }
    }
}

pub struct HttmCopy;

impl HttmCopy {
    pub fn new(src: &Path, dst: &Path) -> HttmResult<()> {
        // create source file reader
        let src_file = File::open(src)?;
        let src_len = src_file.metadata()?.len();

        let dst_file = OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .open(dst)?;
        dst_file.set_len(src_len)?;

        let amt_written = DiffCopy::new(&src_file, &dst_file)?;

        if amt_written != src_len as usize {
            let msg = format!(
                "Amount written (\"{}\") != Source length (\"{}\").  Quitting.",
                amt_written, src_len
            );
            return Err(HttmError::new(&msg).into());
        }

        if GLOBAL_CONFIG.opt_debug {
            DiffCopy::confirm(src, dst)?
        }

        Ok(())
    }
}

struct DiffCopy;

impl DiffCopy {
    fn new(src_file: &File, dst_file: &File) -> HttmResult<usize> {
        if !GLOBAL_CONFIG.opt_no_clones
            && IS_CLONE_COMPATIBLE.load(std::sync::atomic::Ordering::Relaxed)
        {
            let src_fd = src_file.as_fd();
            let dst_fd = dst_file.as_fd();
            let src_len = src_file.metadata()?.len();

            match Self::copy_file_range(src_fd, dst_fd, src_len as usize) {
                Ok(amt_written) if amt_written as u64 == src_len => {
                    if GLOBAL_CONFIG.opt_debug {
                        eprintln!("DEBUG: copy_file_range call successful.");
                    }
                    return Ok(amt_written);
                }
                _ => {
                    IS_CLONE_COMPATIBLE.store(false, std::sync::atomic::Ordering::Relaxed);
                    if GLOBAL_CONFIG.opt_debug {
                        eprintln!(
                            "DEBUG: copy_file_range call unsuccessful.  \
                        IS_CLONE_COMPATIBLE variable has been modified to: \"{:?}\".",
                            IS_CLONE_COMPATIBLE.load(std::sync::atomic::Ordering::Relaxed)
                        );
                    }
                }
            }
        }

        Self::write_no_cow(&src_file, &dst_file)
    }

    #[inline]
    fn write_no_cow(src_file: &File, dst_file: &File) -> HttmResult<usize> {
        // create destination file writer and maybe reader
        // only include dst file reader if the dst file exists
        // otherwise we just write to that location
        let mut src_reader = BufReader::with_capacity(CHUNK_SIZE, src_file);
        let mut dst_reader = BufReader::with_capacity(CHUNK_SIZE, dst_file);
        let mut dst_writer = BufWriter::with_capacity(CHUNK_SIZE, dst_file);

        let dst_exists = DstFileState::exists(dst_file);

        // cur pos - byte offset in file,
        let mut cur_pos = 0u64;

        // return value
        let mut bytes_processed = 0usize;

        loop {
            match src_reader.fill_buf() {
                Ok(src_read) => {
                    // read (size of buffer amt) from src, and dst if it exists
                    let src_amt_read = src_read.len();

                    if src_amt_read == 0 {
                        break;
                    }

                    match dst_exists {
                        DstFileState::DoesNotExist => Self::write_to_offset(
                            &mut dst_writer,
                            src_read,
                            cur_pos,
                            &mut bytes_processed,
                        )?,
                        DstFileState::Exists => {
                            // read same amt from dst file, if it exists, to compare
                            match dst_reader.fill_buf() {
                                Ok(dst_read) => {
                                    if Self::is_same_bytes(src_read, dst_read) {
                                        bytes_processed += src_amt_read;
                                    } else {
                                        Self::write_to_offset(
                                            &mut dst_writer,
                                            src_read,
                                            cur_pos,
                                            &mut bytes_processed,
                                        )?
                                    }

                                    let dst_amt_read = dst_read.len();
                                    dst_reader.consume(dst_amt_read);
                                }
                                Err(err) => match err.kind() {
                                    ErrorKind::Interrupted => continue,
                                    ErrorKind::UnexpectedEof => {
                                        break;
                                    }
                                    _ => return Err(err.into()),
                                },
                            }
                        }
                    };

                    cur_pos += src_amt_read as u64;

                    src_reader.consume(src_amt_read);
                }
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted => continue,
                    ErrorKind::UnexpectedEof => {
                        break;
                    }
                    _ => return Err(err.into()),
                },
            };
        }

        // re docs, both a flush and a sync seem to be required re consistency
        dst_file.sync_data()?;

        Ok(bytes_processed)
    }

    #[inline]
    fn is_same_bytes(a_bytes: &[u8], b_bytes: &[u8]) -> bool {
        let (a_hash, b_hash): (u64, u64) =
            rayon::join(|| Self::hash(a_bytes), || Self::hash(b_bytes));

        a_hash == b_hash
    }

    #[inline]
    fn hash(bytes: &[u8]) -> u64 {
        use std::hash::Hasher;

        let mut hash = ahash::AHasher::default();

        hash.write(bytes);
        hash.finish()
    }

    fn write_to_offset(
        dst_writer: &mut BufWriter<&File>,
        src_read: &[u8],
        cur_pos: u64,
        bytes_processed: &mut usize,
    ) -> HttmResult<()> {
        // seek to current byte offset in dst writer
        let seek_pos = dst_writer.seek(SeekFrom::Start(cur_pos))?;

        if seek_pos != cur_pos {
            let msg = format!("Could not seek to offset in destination file: {}", cur_pos);
            return Err(HttmError::new(&msg).into());
        }

        *bytes_processed += dst_writer.write(src_read)?;

        Ok(())
    }

    #[allow(unreachable_code, unused_variables)]
    fn copy_file_range(
        src_file_fd: BorrowedFd,
        dst_file_fd: BorrowedFd,
        len: usize,
    ) -> HttmResult<usize> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            match nix::fcntl::copy_file_range(src_file_fd, None, dst_file_fd, None, len) {
                Ok(bytes_written) => return Ok(bytes_written),
                Err(err) => match err {
                    nix::errno::Errno::ENOSYS => {
                        return Err(HttmError::new(
                            "Operating system does not support copy_file_ranges.",
                        )
                        .into())
                    }
                    _ => {
                        if GLOBAL_CONFIG.opt_debug {
                            eprintln!("DEBUG: copy_file_range call failed for the following reason: {}\nDEBUG: Falling back to default diff copy behavior.", err);
                        }
                    }
                },
            }
        }
        Err(HttmError::new("Operating system does not support copy_file_ranges.").into())
    }

    fn confirm(src: &Path, dst: &Path) -> HttmResult<()> {
        let src_test =
            CompareVersionsContainer::new(PathData::from(src), &ListSnapsOfType::UniqueContents);
        let dst_test =
            CompareVersionsContainer::new(PathData::from(dst), &ListSnapsOfType::UniqueContents);

        if src_test.is_same_file(&dst_test) {
            eprintln!(
                "DEBUG: Copy successful.  File contents of {} and {} are the same.",
                src.display(),
                dst.display()
            );

            Ok(())
        } else {
            let msg = format!(
                "Copy failed.  File contents of {} and {} are NOT the same.",
                src.display(),
                dst.display()
            );

            Err(HttmError::new(&msg).into())
        }
    }
}
