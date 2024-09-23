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

use crate::data::paths::PathData;
use crate::library::results::{HttmError, HttmResult};
use crate::zfs::run_command::RunZFSCommand;
use crate::{ExecMode, GLOBAL_CONFIG, IN_BUFFER_SIZE};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, ErrorKind, Seek, SeekFrom, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;
use std::process::Command as ExecProcess;
use std::sync::atomic::AtomicBool;
use std::sync::LazyLock;

static IS_CLONE_COMPATIBLE: LazyLock<AtomicBool> = LazyLock::new(|| {
    if let Ok(run_zfs) = RunZFSCommand::new() {
        let Ok(process_output) = ExecProcess::new(&run_zfs.zfs_command).arg("-V").output() else {
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

        if let ExecMode::RollForward(_) = GLOBAL_CONFIG.exec_mode {
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
        let src_file = std::fs::OpenOptions::new().read(true).open(src)?;
        let src_len = src_file.metadata()?.len();

        let mut dst_file = OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .open(dst)?;
        dst_file.set_len(src_len)?;

        match DiffCopy::new(&src_file, &mut dst_file) {
            Ok(_) if GLOBAL_CONFIG.opt_debug => {
                eprintln!("DEBUG: Write to file completed.  Confirmation initiated.");
                DiffCopy::confirm(src, dst)
            }
            Ok(_) => Ok(()),
            Err(err) => Err(err),
        }
    }
}

struct DiffCopy;

impl DiffCopy {
    fn new(src_file: &File, dst_file: &mut File) -> HttmResult<()> {
        let src_len = src_file.metadata()?.len();

        if !GLOBAL_CONFIG.opt_no_clones
            && IS_CLONE_COMPATIBLE.load(std::sync::atomic::Ordering::Relaxed)
        {
            let src_fd = src_file.as_fd();
            let dst_fd = dst_file.as_fd();

            match Self::copy_file_range(src_fd, dst_fd, src_len as usize) {
                Ok(_) => {
                    if GLOBAL_CONFIG.opt_debug {
                        eprintln!("DEBUG: copy_file_range call successful.");
                    }
                    return Ok(());
                }
                Err(err) => {
                    IS_CLONE_COMPATIBLE.store(false, std::sync::atomic::Ordering::Relaxed);
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
        use std::hash::Hasher;

        let mut hash = ahash::AHasher::default();

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
        len: usize,
    ) -> HttmResult<()> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let mut amt_written = 0u64;

            // copy_file_range needs to be run in a loop as it is interruptible
            match nix::fcntl::copy_file_range(
                src_file_fd,
                Some(&mut (amt_written as i64)),
                dst_file_fd,
                Some(&mut (amt_written as i64)),
                len,
            ) {
                // However,	a return of zero  for  a  non-zero  len  argument
                // indicates that the offset for infd is at or beyond EOF.
                Ok(bytes_written) if bytes_written == 0usize && len != 0usize => {
                    return Err(ErrorKind::UnexpectedEof)
                }
                Ok(bytes_written) => {
                    amt_written += bytes_written as u64;

                    if amt_written == len as u64 {
                        return Ok(());
                    }

                    if amt_written < len as u64 {
                        //  Upon successful completion, copy_file_range() will  return  the  number  of  bytes  copied
                        //  between files.  This could be less than the length originally requested.
                        return Ok(());
                    }

                    if amt_written > len as u64 {
                        return Err(HttmError::new("Amount written larger than file len.").into());
                    }
                }
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
        let src_test = PathData::from(src);
        let dst_test = PathData::from(dst);

        if src_test.is_same_file_contents(&dst_test) {
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
