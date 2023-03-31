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

// this module is a re-implementation of the diff_copy() method, as used by lms_lib.
// this was/is done for both performance and binary size reasons.
//
// though I am fairly certain this re-implementation of their API is fair use
// I've reproduced their license, as of 3/30/2023, verbatim below:

// MIT License

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

const CHUNK_SIZE: usize = 8_192;

pub fn diff_copy(src: &Path, dst: &Path) -> HttmResult<()> {
    if !Path::new(&dst).exists() {
        let _ = std::fs::copy(src, dst)?;
        return Ok(());
    }

    let src_file = File::open(src)?;
    let mut src_reader = BufReader::with_capacity(CHUNK_SIZE, &src_file);

    let dst_file = OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .open(dst)?;
    dst_file.set_len(src_file.metadata()?.len())?;

    let mut dst_reader = BufReader::with_capacity(CHUNK_SIZE, &dst_file);
    let mut dst_writer = BufWriter::with_capacity(CHUNK_SIZE, &dst_file);

    loop {
        let mut src_buffer = [0; CHUNK_SIZE];
        let mut dest_buffer = [0; CHUNK_SIZE];

        if src_reader.read(&mut src_buffer)? == 0 {
            break;
        }
        let _ = dst_reader.read(&mut dest_buffer)?;

        if !is_same_bytes(&src_buffer, &dest_buffer) {
            let _ = dst_writer.write(&src_buffer)?;
        } else {
            dst_writer.seek(SeekFrom::Current(CHUNK_SIZE as i64))?;
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
