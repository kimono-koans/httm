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

                if src_amt_read == CHUNK_SIZE {
                    if *COW_COMPATIBLE && cfg!(target_os = "linux") {
                        #[cfg(target_os = "linux")]
                        let amd_written = write_cow(src_file, dst_file, cur_pos, src_amt_read)?;
                        #[cfg(target_os = "linux")]

                    } else {
                        dst_writer.write(&src_buffer)?
                    }
                } else {
                    if *COW_COMPATIBLE && cfg!(target_os = "linux") {
                        #[cfg(target_os = "linux")]
                        let amd_written = write_cow(src_file, dst_file, cur_pos, src_amt_read)?;
                        #[cfg(target_os = "linux")]

                    } else {
                        let range: &[u8] = &src_buffer[0..src_amt_read];
                        dst_writer.write(range)?
                    }
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
    let (a_hash, b_hash): (u32, u32) = rayon::join(|| hash(a_bytes), || hash(b_bytes));

    a_hash == b_hash
}

#[inline]
fn hash(bytes: &[u8; CHUNK_SIZE]) -> u32 {
    let mut hash = Adler32::new();

    hash.write(bytes);
    hash.finish()
}

#[cfg(target_os = "linux")]
fn write_cow(src_file: File, dst_file: File, offset: i64, len: usize) -> HttmResult<usize> {
    use nix::fcntl::copy_file_range;
    use std::os::unix::io::AsRawFd;

    let mut src_mutable_offset = offset;
    let mut dst_mutable_offset = offset;

    let bytes_written = copy_file_range(
        src_file.as_raw_fd(),
        Some(&mut src_mutable_offset),
        dst_file.as_raw_fd(),
        Some(&mut dst_mutable_offset),
        len,
    )?;

    Ok(bytes_written)
}

use semver::Version;
use once_cell::sync::Lazy;

pub fn version() -> Option<Version> {
    use nix::sys::utsname::*;

    uname().ok().map(|sysinfo| {
        Version::parse(sysinfo.release().to_string_lossy().as_ref()).ok()
    }).flatten()
}

static COW_COMPATIBLE: Lazy<bool> = Lazy::new(|| {
    let version = version().unwrap();

    if version.major >= 4 && version.minor >= 5 {
        return true
    }

    false
});
