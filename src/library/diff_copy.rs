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

use simd_adler32::Adler32;

use crate::library::results::HttmResult;

const CHUNK_SIZE: usize = 65_536;

pub fn diff_copy(src: &Path, dst: &Path) -> HttmResult<()> {
    // create source file reader
    let src_file = File::open(src)?;
    let mut src_reader = BufReader::with_capacity(CHUNK_SIZE, &src_file);

    // create destination if it doesn't exist
    let dst_file = OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .open(dst)?;
    let src_len = src_file.metadata()?.len();
    dst_file.set_len(src_len)?;

    // create destination file writer and maybe reader
    // only include dest file reader if the dest file exists
    // otherwise we just write to that location
    let mut opt_just_write = if dst.exists() {
        None
    } else {
        let dst_reader = BufReader::with_capacity(CHUNK_SIZE, &dst_file);
        Some(dst_reader)
    };
    let mut dst_writer = BufWriter::with_capacity(CHUNK_SIZE, &dst_file);

    // cur pos - byte offset in file,
    let mut cur_pos = 0u64;
    // buffers are the reusable buffers for reads to, writes from src and dest
    let mut src_buffer = [0; CHUNK_SIZE];
    let mut dest_buffer = [0; CHUNK_SIZE];

    while cur_pos < src_len {
        // read (size of buffer amt) from src, and dest if it exists
        let src_amt_read = src_reader.read(&mut src_buffer)?;

        // if nothing left to read from file, break
        if src_amt_read == 0 {
            continue;
        }

        // read same amt from dest file, if it exists, to compare
        if let Some(ref mut dst_reader) = opt_just_write {
            let _ = dst_reader.read(&mut dest_buffer)?;
        }

        // write if dest doesn't exist or src, or if src and dest buffers do not match
        if opt_just_write.is_some() || !is_same_bytes(&src_buffer, &dest_buffer) {
            // seek to current byte offset in dest writer
            let seek_pos = dst_writer.seek(SeekFrom::Start(cur_pos))?;

            assert!(seek_pos == cur_pos);

            // write only amt read - imagine we read less than the amt of the buffer
            // don't write past the end of the file with junk data at the end of the buffer
            let range: &[u8] = &src_buffer[0..src_amt_read];
            let amt_written = dst_writer.write(range)?;

            assert!(amt_written == src_amt_read);

            // set current byte offset to either amt written or read if no write occurred
            cur_pos += amt_written as u64;
        } else {
            cur_pos += src_amt_read as u64;
        }
    }

    // re docs, both a flush and a sync seem to be required re consistency
    dst_writer.flush()?;
    dst_file.sync_data()?;

    Ok(())
}

#[inline]
fn is_same_bytes(a_bytes: &[u8; CHUNK_SIZE], b_bytes: &[u8; CHUNK_SIZE]) -> bool {
    let (a_hash, b_hash): (u32, u32) = rayon::join(|| hash(a_bytes), || hash(b_bytes));

    a_hash == b_hash
}

#[inline]
fn hash(bytes: &[u8; CHUNK_SIZE]) -> u32 {
    let mut hash = Adler32::default();

    hash.write(bytes);
    hash.finish()
}
