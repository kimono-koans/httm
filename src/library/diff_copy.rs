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
use crate::library::results::{
    HttmError,
    HttmResult,
};
use crate::zfs::run_command::RunZFSCommand;
use crate::{
    ExecMode,
    GLOBAL_CONFIG,
    IN_BUFFER_SIZE,
    InteractiveMode,
};
use indicatif::{
    ProgressBar,
    ProgressStyle,
};
use std::borrow::Cow;
use std::fs::{
    File,
    OpenOptions,
};
use std::io::{
    BufRead,
    BufReader,
    BufWriter,
    ErrorKind,
    Seek,
    SeekFrom,
    Write,
};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::os::fd::AsFd;
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
    fn new(dst_file: &File) -> Self {
        if dst_file.metadata().is_ok() {
            DstFileState::Exists
        } else {
            DstFileState::DoesNotExist
        }
    }
}

pub struct HttmCopy<'a> {
    src_path: &'a Path,
    dst_path: &'a Path,
    src_len: u64,
    opt_bar: Option<ProgressBar>,
}

impl<'a> HttmCopy<'a> {
    pub fn new(src_path: &'a Path, dst_path: &'a Path) -> Self {
        let file_name = src_path.file_name().unwrap_or_default().to_string_lossy();
        let src_len = src_path.symlink_metadata().unwrap().len();

        let opt_bar = Self::opt_bar(file_name, src_len);

        Self {
            src_path,
            dst_path,
            src_len,
            opt_bar,
        }
    }

    pub fn exec(self) -> HttmResult<()> {
        if !GLOBAL_CONFIG.opt_no_clones
            && IS_CLONE_COMPATIBLE.load(std::sync::atomic::Ordering::Relaxed)
        {
            match CloneCopy::try_from(&self)?.exec() {
                Ok(_) => {
                    if GLOBAL_CONFIG.opt_debug {
                        eprintln!("DEBUG: copy_file_range call successful.");
                        eprintln!("DEBUG: Confirmation initiated.");
                        Self::confirm(self.src_path, self.dst_path)?;
                        eprintln!("DEBUG: Confirmation completed successfully.");
                    }

                    return Ok(());
                }
                Err(err) => {
                    IS_CLONE_COMPATIBLE.store(false, std::sync::atomic::Ordering::Relaxed);
                    if GLOBAL_CONFIG.opt_debug {
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
        }

        DiffCopy::try_from(&self)?.exec()?;

        if GLOBAL_CONFIG.opt_debug {
            eprintln!("DEBUG: DiffCopy successful.");
            eprintln!("DEBUG: Confirmation initiated.");
            Self::confirm(self.src_path, self.dst_path)?;
            eprintln!("DEBUG: Confirmation completed successfully.");
        }

        Ok(())
    }

    fn opt_bar(file_name: Cow<str>, len: u64) -> Option<ProgressBar> {
        let bar = match GLOBAL_CONFIG.exec_mode {
            ExecMode::Interactive(InteractiveMode::Restore(_)) => ProgressBar::new(len).with_style(
                ProgressStyle::with_template("[{decimal_total_bytes}] {bar:40.cyan/blue} {msg}")
                    .ok()?,
            ),
            _ if len.gt(&1_000_000_000) => ProgressBar::new(len).with_style(
                ProgressStyle::with_template("[{decimal_total_bytes}] {bar:40.cyan/blue} {msg}")
                    .ok()?,
            ),
            _ => return None,
        };

        bar.set_message(file_name.into_owned());

        Some(bar)
    }

    fn finish_and_clear_bar(&self) {
        if let Some(bar) = &self.opt_bar {
            bar.finish_and_clear()
        }
    }

    fn increment_bar(&self, bytes_written: u64) {
        if let Some(bar) = &self.opt_bar {
            bar.inc(bytes_written as u64)
        }
    }

    pub fn confirm(src: &Path, dst: &Path) -> HttmResult<()> {
        if is_same_file_contents(src, dst) {
            return Ok(());
        }

        let description = format!(
            "Copy failed.  File contents of {} and {} are NOT the same.",
            src.display(),
            dst.display()
        );

        HttmError::from(description).into()
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
        use std::hash::{
            BuildHasher,
            Hasher,
        };

        let s = FixedState::default();
        let mut hash = s.build_hasher();

        hash.write(bytes);
        hash.finish()
    }
}

#[allow(dead_code)]
pub struct CloneCopy<'a> {
    src_file: File,
    dst_file: File,
    httm_copy: &'a HttmCopy<'a>,
}

impl<'a> TryFrom<&'a HttmCopy<'a>> for CloneCopy<'a> {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: &'a HttmCopy) -> HttmResult<CloneCopy<'a>> {
        // create source file reader
        let src_file = std::fs::OpenOptions::new()
            .read(true)
            .open(value.src_path)?;

        let dst_file = OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .open(value.dst_path)?;

        dst_file.set_len(value.src_len)?;

        Ok(Self {
            src_file,
            dst_file,
            httm_copy: value,
        })
    }
}

impl<'a> CloneCopy<'a> {
    fn exec(mut self) -> HttmResult<()> {
        self.copy_file_range()?;

        // re docs, both a flush and a sync seem to be required re consistency
        self.dst_file.flush()?;
        self.dst_file.sync_data()?;

        Ok(())
    }

    #[allow(unreachable_code, unused_variables)]
    #[inline]
    fn copy_file_range(&self) -> HttmResult<()> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let src_fd = self.src_file.as_fd();
            let dst_fd = self.dst_file.as_fd();

            let mut amt_written = 0u64;
            let len = self.httm_copy.src_len;
            let mut remainder = len.clone() as usize;

            while remainder > 0 {
                let mut off_src = amt_written as i64;
                let mut off_dst = off_src.clone();

                match nix::fcntl::copy_file_range(
                    src_fd,
                    Some(&mut off_src),
                    dst_fd,
                    Some(&mut off_dst),
                    remainder,
                ) {
                    // a return of zero for a non-zero len argument
                    // indicates that the offset for infd is at or beyond EOF.
                    Ok(bytes_written) if bytes_written == 0 && remainder != 0 => {
                        return Err(
                            HttmError::new("Offset for fd_in at or beyond end of file").into()
                        );
                    }
                    Ok(bytes_written) => {
                        amt_written += bytes_written as u64;
                        remainder -= bytes_written as usize;

                        self.httm_copy.increment_bar(bytes_written as u64);
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

            assert_eq!(amt_written, len);

            self.httm_copy.finish_and_clear_bar();

            return Ok(());
        }

        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        HttmError::new("Operating system does not support copy_file_ranges.").into()
    }
}

pub struct DiffCopy<'a> {
    src_file: File,
    dst_file: File,
    httm_copy: &'a HttmCopy<'a>,
}

impl<'a> TryFrom<&'a HttmCopy<'a>> for DiffCopy<'a> {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: &'a HttmCopy) -> HttmResult<DiffCopy<'a>> {
        // create source file reader
        let src_file = std::fs::OpenOptions::new()
            .read(true)
            .open(value.src_path)?;

        let dst_file = OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .open(value.dst_path)?;

        dst_file.set_len(value.src_len)?;

        Ok(Self {
            src_file,
            dst_file,
            httm_copy: value,
        })
    }
}

impl<'a> DiffCopy<'a> {
    fn exec(mut self) -> HttmResult<()> {
        self.write_no_cow()?;

        // re docs, both a flush and a sync seem to be required re consistency
        self.dst_file.flush()?;
        self.dst_file.sync_data()?;

        Ok(())
    }

    #[inline]
    fn write_no_cow(&self) -> HttmResult<()> {
        // create destination file writer and maybe reader
        // only include dst file reader if the dst file exists
        // otherwise we just write to that location
        let mut src_reader = BufReader::with_capacity(IN_BUFFER_SIZE, &self.src_file);
        let mut dst_reader = BufReader::with_capacity(IN_BUFFER_SIZE, &self.dst_file);
        let mut dst_writer = BufWriter::with_capacity(IN_BUFFER_SIZE, &self.dst_file);

        let dst_exists = DstFileState::new(&self.dst_file);

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
                                    if !HttmCopy::is_same_bytes(src_read, dst_read) {
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

                    self.httm_copy.increment_bar(src_amt_read as u64);

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

        self.httm_copy.finish_and_clear_bar();

        Ok(())
    }

    #[inline]
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
}
