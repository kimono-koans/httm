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
use std::io::Read;
use std::path::Path;
use std::os::unix::fs::FileExt;

use simd_adler32::Adler32;

use crate::library::results::HttmError;
use crate::library::results::HttmResult;

const CHUNK_SIZE: usize = 10_000;

pub fn diff_copy(src: &Path, dst: &Path) -> HttmResult<()> {
    let mut just_write = false;

    if !dst.exists() {
        just_write = true;
    }

    let src_file = File::open(src)?;
    let mut src_reader = BufReader::with_capacity(CHUNK_SIZE, &src_file);

    let dst_file = OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .open(dst)?;

    let src_len = src_file.metadata()?.len();
    dst_file.set_len(src_len)?;

    let mut dst_reader = BufReader::with_capacity(CHUNK_SIZE, &dst_file);

    let mut cur_pos = 0u64;
    let mut total_amt_read = 0u64;

    let mut src_buffer = [0; CHUNK_SIZE];
    let mut dest_buffer = [0; CHUNK_SIZE];

    loop {
        let src_amt_read = src_reader.read(&mut src_buffer)?;
        if src_amt_read == 0 {
            break;
        }
        let _ = dst_reader.read(&mut dest_buffer)?;
        total_amt_read += src_amt_read as u64;

        if just_write || !is_same_bytes(&src_buffer, &dest_buffer) {            
            let amt_written = if src_amt_read == CHUNK_SIZE {
                dst_file.write_at(&src_buffer, cur_pos)?
            } else {
                let range: &[u8] = &src_buffer[0..src_amt_read];
                dst_file.write_at(range, cur_pos)?
            };

            cur_pos += amt_written as u64;
        } else {
            cur_pos += src_amt_read as u64;
        }
    }

    assert!(cur_pos == total_amt_read);

    dst_file.sync_data()?;

    if dst.is_file() {
        let dst_len = dst_file.metadata()?.len();
        if src_len != dst_len {
            return Err(HttmError::new("src_len and dst_len do not match.").into());
        }
    }

    Ok(())
}

#[inline]
fn is_same_bytes(a_bytes: &[u8; CHUNK_SIZE], b_bytes: &[u8; CHUNK_SIZE]) -> bool {
    let (a_hash, b_hash): (u32, u32) = rayon::join(|| hash(a_bytes), || hash(b_bytes));

    a_hash == b_hash
}

#[inline]
fn hash(bytes: &[u8; CHUNK_SIZE]) -> u32 {
    let mut hash = Adler32::new();

    hash.write(bytes);
    hash.finish()
}
