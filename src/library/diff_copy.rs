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

use simd_adler32::Adler32;

use crate::library::results::HttmResult;

const CHUNK_SIZE: usize = 65_536;
const NONE_READ: usize = 0;

enum DstFileState {
    Exists,
    DoesNotExist(usize),
}

pub fn diff_copy(src: &Path, dst: &Path) -> HttmResult<()> {
    // create source file reader
    let src_file = File::open(src)?;
    let mut src_reader = BufReader::with_capacity(CHUNK_SIZE, &src_file);

    // create destination if it doesn't exist
    let dst_exists = if dst.exists() {
        DstFileState::Exists
    } else {
        DstFileState::DoesNotExist(NONE_READ)
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

                let dst_amt_read = match dst_exists {
                    DstFileState::DoesNotExist(none_read) => {
                        // seek to current byte offset in dst writer
                        let _seek_pos = dst_writer.seek(SeekFrom::Start(cur_pos))?;

                        dst_writer.write_all(src_read)?;

                        none_read
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

                                dst_amt_read
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
                dst_reader.consume(dst_amt_read);
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
