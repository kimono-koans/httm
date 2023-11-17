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
use std::path::Path;

use clone_file::clone_file;
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
    // attempt zero copy clone, unless user has specified no clones
    if !GLOBAL_CONFIG.opt_no_clones {
        match clone_file(src, dst) {
            Ok(_) => return Ok(()),
            Err(err) => {
                if GLOBAL_CONFIG.opt_debug {
                    eprintln!("DEBUG: File clone failed for the following reason: {}\nDEBUG: Falling back to default diff copy behavior.", err);
                }
            }
        }
    }

    // create source file reader
    let src_file = File::open(src)?;
    let mut src_reader = BufReader::with_capacity(CHUNK_SIZE, &src_file);

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

    let src_len = src_file.metadata()?.len();
    dst_file.set_len(src_len)?;

    // create destination file writer and maybe reader
    // only include dst file reader if the dst file exists
    // otherwise we just write to that location
    let mut dst_reader = BufReader::with_capacity(CHUNK_SIZE, &dst_file);
    let mut dst_writer = BufWriter::with_capacity(CHUNK_SIZE, &dst_file);

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
                        // seek to current byte offset in dst writer
                        let _seek_pos = dst_writer.seek(SeekFrom::Start(cur_pos))?;

                        dst_writer.write_all(src_read)?;
                    }
                    DstFileState::Exists => {
                        // read same amt from dst file, if it exists, to compare
                        match dst_reader.fill_buf() {
                            Ok(dst_read) => {
                                let dst_amt_read = dst_read.len();

                                if !is_same_bytes(src_read, dst_read) {
                                    // seek to current byte offset in dst writer
                                    let _seek_pos = dst_writer.seek(SeekFrom::Start(cur_pos))?;

                                    dst_writer.write_all(src_read)?
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
    dst_writer.flush()?;
    dst_file.sync_data()?;

    if GLOBAL_CONFIG.opt_debug {
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
        } else {
            let msg = format!(
                "Copy failed.  File contents of {} and {} are NOT the same.",
                src.display(),
                dst.display()
            );
            return Err(HttmError::new(&msg).into());
        }
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
