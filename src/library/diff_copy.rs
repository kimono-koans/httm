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

use std::fs::File;
use std::fs::OpenOptions;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::ErrorKind;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use simd_adler32::Adler32;

use crate::config::generate::ListSnapsOfType;
use crate::data::paths::CompareVersionsContainer;
use crate::data::paths::PathData;
use crate::library::results::HttmResult;
use crate::GLOBAL_CONFIG;

use super::results::HttmError;

const CHUNK_SIZE: usize = 65_536;

enum DstFileState {
    Exists,
    DoesNotExist,
}

pub fn diff_copy(src: &Path, dst: &Path) -> HttmResult<()> {
    // create source file reader
    let src_file = File::open(src)?;
    let src_fd = src_file.as_fd();
    let src_len = src_file.metadata()?.len();

    // create destination if it doesn't exist
    let dst_exists = if dst.exists() {
        DstFileState::Exists
    } else {
        DstFileState::DoesNotExist
    };

    let dst_file = OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .open(dst)?;
    let dst_fd = dst_file.as_fd();
    dst_file.set_len(src_len)?;

    static IS_CLONE_COMPATIBLE: AtomicBool = AtomicBool::new(true);

    let amt_written = if GLOBAL_CONFIG.opt_no_clones
        || !IS_CLONE_COMPATIBLE.load(std::sync::atomic::Ordering::Relaxed)
    {
        write_loop(&src_file, &dst_file, dst_exists)?
    } else {
        match httm_copy_file_range(src_fd, dst_fd, 0 as i64, src_len as usize) {
            Ok(amt_written) if amt_written as u64 == src_len => {
                if GLOBAL_CONFIG.opt_debug {
                    eprintln!("DEBUG: copy_file_range call successful.");
                }
                amt_written
            }
            _ => {
                IS_CLONE_COMPATIBLE.store(false, std::sync::atomic::Ordering::Relaxed);
                write_loop(&src_file, &dst_file, dst_exists)?
            }
        }
    };

    if amt_written != src_len as usize {
        let msg = format!(
            "Amount written (\"{}\") != Source length (\"{}\").  Quitting.",
            amt_written, src_len
        );
        return Err(HttmError::new(&msg).into());
    }

    if GLOBAL_CONFIG.opt_debug {
        confirm(src, dst)?
    }

    Ok(())
}

#[inline]
fn is_same_bytes(a_bytes: &[u8], b_bytes: &[u8]) -> bool {
    let (a_hash, b_hash): (u32, u32) = rayon::join(|| hash(a_bytes), || hash(b_bytes));

    a_hash == b_hash
}

#[inline]
fn hash(bytes: &[u8]) -> u32 {
    let mut hash = Adler32::default();

    hash.write(bytes);
    hash.finish()
}

fn write_loop(src_file: &File, dst_file: &File, dst_exists: DstFileState) -> HttmResult<usize> {
    // create destination file writer and maybe reader
    // only include dst file reader if the dst file exists
    // otherwise we just write to that location
    let mut src_reader = BufReader::with_capacity(CHUNK_SIZE, src_file);
    let mut dst_reader = BufReader::with_capacity(CHUNK_SIZE, dst_file);
    let mut dst_writer = BufWriter::with_capacity(CHUNK_SIZE, dst_file);

    // cur pos - byte offset in file,
    let mut cur_pos = 0u64;

    // return value
    let mut amt_written = 0usize;

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
                        // seek to current byte offset in dst writer
                        let seek_pos = dst_writer.seek(SeekFrom::Start(cur_pos))?;

                        if seek_pos != cur_pos {
                            continue;
                        }

                        amt_written += dst_writer.write(src_read)?;
                    }
                    DstFileState::Exists => {
                        // read same amt from dst file, if it exists, to compare
                        match dst_reader.fill_buf() {
                            Ok(dst_read) => {
                                let dst_amt_read = dst_read.len();

                                if !is_same_bytes(src_read, dst_read) {
                                    // seek to current byte offset in dst writer
                                    let seek_pos = dst_writer.seek(SeekFrom::Start(cur_pos))?;

                                    if seek_pos != cur_pos {
                                        continue;
                                    }

                                    amt_written += dst_writer.write(src_read)?;
                                }

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

    Ok(amt_written)
}

#[allow(unreachable_code, unused_variables)]
fn httm_copy_file_range(
    src_file_fd: BorrowedFd,
    dst_file_fd: BorrowedFd,
    offset: i64,
    len: usize,
) -> HttmResult<usize> {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        use nix::fcntl::copy_file_range;

        let mut src_mutable_offset = offset;
        let mut dst_mutable_offset = offset;

        let res = copy_file_range(
            src_file_fd,
            Some(&mut src_mutable_offset),
            dst_file_fd,
            Some(&mut dst_mutable_offset),
            len,
        );

        match res {
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
