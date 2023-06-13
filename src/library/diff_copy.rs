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
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;

use ahash::AHasher;
use std::hash::Hasher;

use crate::library::results::HttmResult;

use super::results::HttmError;

const CHUNK_SIZE: usize = 65_536;

pub fn diff_copy(src: &Path, dst: &Path) -> HttmResult<()> {
    let src_file = File::open(src)?;
    let mut src_reader = BufReader::with_capacity(CHUNK_SIZE, &src_file);

    let dst_file = OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .open(dst)?;
    let src_len = src_file.metadata()?.len();
    dst_file.set_len(src_len)?;
    let mut opt_just_write = if dst.exists() {
        None
    } else {
        let dst_reader = BufReader::with_capacity(CHUNK_SIZE, &dst_file);
        Some(dst_reader)
    };
    let mut dst_writer = BufWriter::with_capacity(CHUNK_SIZE, &dst_file);

    let mut cur_pos = 0u64;
    let mut src_buffer = [0; CHUNK_SIZE];
    let mut dest_buffer = [0; CHUNK_SIZE];

    loop {
        let src_amt_read = src_reader.read(&mut src_buffer)?;
        if src_amt_read == 0 {
            break;
        }
        if let Some(ref mut dst_reader) = opt_just_write {
            let _ = dst_reader.read(&mut dest_buffer)?;
        }

        if opt_just_write.is_some() || !is_same_bytes(&src_buffer, &dest_buffer) {
            let seek_pos = dst_writer.seek(SeekFrom::Start(cur_pos))?;

            if seek_pos != cur_pos {
                return Err(HttmError::new("Seek offset did not match requested offset.").into());
            }

            let amt_written = if src_amt_read == CHUNK_SIZE {
                dst_writer.write(&src_buffer)?
            } else {
                let range: &[u8] = &src_buffer[0..src_amt_read];
                dst_writer.write(range)?
            };

            if amt_written != src_amt_read {
                return Err(HttmError::new(
                    "Amount of bytes read did not match amount of bytes written.",
                )
                .into());
            }

            cur_pos += amt_written as u64;
        } else {
            cur_pos += src_amt_read as u64;
        }
    }

    dst_writer.flush()?;
    dst_file.sync_data()?;

    Ok(())
}

#[inline]
fn is_same_bytes(a_bytes: &[u8; CHUNK_SIZE], b_bytes: &[u8; CHUNK_SIZE]) -> bool {
    let (a_hash, b_hash): (u64, u64) = rayon::join(|| hash(a_bytes), || hash(b_bytes));

    a_hash == b_hash
}

#[inline]
fn hash(bytes: &[u8; CHUNK_SIZE]) -> u64 {
    let mut hash = AHasher::default();

    hash.write(bytes);
    hash.finish()
}
