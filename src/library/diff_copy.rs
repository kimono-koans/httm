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

use crate::library::file_ops::is_same_file_contents;
use crate::library::results::{HttmError, HttmResult};
use crate::zfs::run_command::RunZFSCommand;
use crate::{GLOBAL_CONFIG, IN_BUFFER_SIZE};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, ErrorKind, Seek, SeekFrom, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;
use std::sync::LazyLock;
use std::sync::atomic::AtomicBool;

static IS_CLONE_COMPATIBLE: LazyLock<AtomicBool> = LazyLock::new(|| {
    let Ok(zfs_cmd) = RunZFSCommand::new() else {
        return AtomicBool::new(false);
    };

    match zfs_cmd.version() {
        Err(_) => return AtomicBool::new(false),
        Ok(stdout)
            if stdout.contains("zfs-2.2.0")
                || stdout.contains("zfs-kmod-2.2.0")
                || stdout.contains("zfs-2.2.1")
                || stdout.contains("zfs-kmod-2.2.1")
                || stdout.contains("zfs-2.2-")
                || stdout.contains("zfs-kmod-2.2-") =>
        {
            return AtomicBool::new(false);
        }
        Ok(_) => return AtomicBool::new(true),
    }
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
        let src_file = std::fs::OpenOptions::new().read(true).open(src)?;
        let src_len = src.symlink_metadata()?.len();

        let mut dst_file = OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .open(dst)?;

        dst_file.set_len(src_len)?;

        match DiffCopy::new(&src_file, &mut dst_file) {
            Ok(_) => {
                if GLOBAL_CONFIG.opt_no_clones && !GLOBAL_CONFIG.opt_debug {
                    return Ok(());
                }

                if GLOBAL_CONFIG.opt_debug {
                    eprintln!("DEBUG: Write to file completed.  Confirmation initiated.");
                }

                match DiffCopy::confirm(src, dst) {
                    Ok(_) => Ok(()),
                    Err(err) => {
                        if !IS_CLONE_COMPATIBLE.load(std::sync::atomic::Ordering::Relaxed) {
                            return Err(err);
                        }

                        eprintln!("WARN: Could not confirm copy_file_range: {}", err);

                        // IS_CLONE_COMPATIBLE.store(false, std::sync::atomic::Ordering::Relaxed);

                        DiffCopy::write_no_cow(&src_file, &dst_file)
                    }
                }
            }
            Err(err) => Err(err),
        }
    }
}

pub struct DiffCopy;

impl DiffCopy {
    fn new(src_file: &File, dst_file: &mut File) -> HttmResult<()> {
        let src_len = src_file.metadata()?.len();

        if !GLOBAL_CONFIG.opt_no_clones
            && IS_CLONE_COMPATIBLE.load(std::sync::atomic::Ordering::Relaxed)
        {
            let src_fd = src_file.as_fd();
            let dst_fd = dst_file.as_fd();

            match Self::copy_file_range(src_fd, dst_fd, src_len) {
                Ok(_) => {
                    if GLOBAL_CONFIG.opt_debug {
                        eprintln!("DEBUG: copy_file_range call successful.");
                    }

                    // re docs, both a flush and a sync seem to be required re consistency
                    dst_file.flush()?;
                    dst_file.sync_data()?;

                    return Ok(());
                }
                Err(err) => {
                    // IS_CLONE_COMPATIBLE.store(false, std::sync::atomic::Ordering::Relaxed);
                    if GLOBAL_CONFIG.opt_debug {
                        eprintln!(
                                "DEBUG: copy_file_range call unsuccessful for the following reason: \"{:?}\".\n
                                DEBUG: Retrying a conventional diff copy.",
                                err
                            );
                    }
                }
            }
        }

        Self::write_no_cow(&src_file, &dst_file)?;

        // re docs, both a flush and a sync seem to be required re consistency
        dst_file.flush()?;
        dst_file.sync_data()?;

        Ok(())
    }

    #[inline]
    fn write_no_cow(src_file: &File, dst_file: &File) -> HttmResult<()> {
        // create destination file writer and maybe reader
        // only include dst file reader if the dst file exists
        // otherwise we just write to that location
        let mut src_reader = BufReader::with_capacity(IN_BUFFER_SIZE, src_file);
        let mut dst_reader = BufReader::with_capacity(IN_BUFFER_SIZE, dst_file);
        let mut dst_writer = BufWriter::with_capacity(IN_BUFFER_SIZE, dst_file);

        let dst_exists = DstFileState::exists(dst_file);

        // cur pos - byte offset in file,
        let mut cur_pos = 0u64;

        loop {
            match src_reader.fill_buf() {
                Ok(src_read) => {
                    // read (size of buffer amt) from src, and dst if it exists
                    let src_amt_read = src_read.len();

                    if src_amt_read == 0 {
                        break;
                    }

                    match dst_exists {
                        DstFileState::DoesNotExist => {
                            Self::write_to_offset(&mut dst_writer, src_read, cur_pos)?;
                        }
                        DstFileState::Exists => {
                            // read same amt from dst file, if it exists, to compare
                            match dst_reader.fill_buf() {
                                Ok(dst_read) => {
                                    if !Self::is_same_bytes(src_read, dst_read) {
                                        Self::write_to_offset(&mut dst_writer, src_read, cur_pos)?
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

        Ok(())
    }

    #[inline]
    fn is_same_bytes(a_bytes: &[u8], b_bytes: &[u8]) -> bool {
        let (a_hash, b_hash): (u64, u64) =
            rayon::join(|| Self::hash(a_bytes), || Self::hash(b_bytes));

        a_hash == b_hash
    }

    #[inline]
    fn hash(bytes: &[u8]) -> u64 {
        use foldhash::quality::FixedState;
        use std::hash::{BuildHasher, Hasher};

        let s = FixedState::default();
        let mut hash = s.build_hasher();

        hash.write(bytes);
        hash.finish()
    }

    fn write_to_offset(
        dst_writer: &mut BufWriter<&File>,
        src_read: &[u8],
        cur_pos: u64,
    ) -> HttmResult<()> {
        // seek to current byte offset in dst writer
        dst_writer.seek(SeekFrom::Start(cur_pos))?;
        dst_writer.write_all(src_read)?;

        Ok(())
    }

    #[allow(unreachable_code, unused_variables)]
    fn copy_file_range(
        src_file_fd: BorrowedFd,
        dst_file_fd: BorrowedFd,
        len: u64,
    ) -> HttmResult<()> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let mut amt_written = 0u64;
            let mut remainder = len as usize;

            while remainder > 0 {
                let mut off_src = amt_written as i64;
                let mut off_dst = off_src.clone();

                match nix::fcntl::copy_file_range(
                    src_file_fd,
                    Some(&mut off_src),
                    dst_file_fd,
                    Some(&mut off_dst),
                    remainder,
                ) {
                    // a return of zero for a non-zero len argument
                    // indicates that the offset for infd is at or beyond EOF.
                    Ok(bytes_written) if bytes_written == 0 && remainder != 0 => break,
                    Ok(bytes_written) => {
                        amt_written += bytes_written as u64;
                        remainder = len.saturating_sub(amt_written) as usize;

                        if amt_written > len {
                            return Err(
                                HttmError::new("Amount written larger than file len.").into()
                            );
                        }
                    }
                    Err(err) => match err {
                        nix::errno::Errno::ENOSYS => {
                            return HttmError::new(
                                "Operating system does not support copy_file_ranges.",
                            )
                            .into();
                        }
                        _ => {
                            if GLOBAL_CONFIG.opt_debug {
                                eprintln!(
                                    "DEBUG: copy_file_range call failed for the following reason: {}\nDEBUG: Falling back to default diff copy behavior.",
                                    err
                                );
                            }

                            return Err(Box::new(err));
                        }
                    },
                }
            }

            return Ok(());
        }

        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        HttmError::new("Operating system does not support copy_file_ranges.").into()
    }

    pub fn confirm(src: &Path, dst: &Path) -> HttmResult<()> {
        if is_same_file_contents(src, dst) {
            Ok(())
        } else {
            let description = format!(
                "Copy failed.  File contents of {} and {} are NOT the same.",
                src.display(),
                dst.display()
            );

            HttmError::from(description).into()
        }
    }
}
