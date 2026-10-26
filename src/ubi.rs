// SPDX-License-Identifier: GPL-2.0-only

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

const EC_HDR_MAGIC: [u8; 4] = *b"UBI#";
const VID_HDR_MAGIC: [u8; 4] = *b"UBI!";
const LAYOUT_VOLUME_ID: u32 = 0x7fff_efff;
const EC_HDR_SIZE: usize = 64;
const VID_HDR_SIZE: usize = 16;

const VTBL_RECORD_SIZE: usize = 172;
const VTBL_RECORDS: usize = 128;
const VTBL_NAME_OFFSET: usize = 16;
const VTBL_NAME_MAX: usize = 128;

fn be_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_exact_at(file: &File, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
    let mut done = 0;
    while done < buffer.len() {
        let count = file.read_at(&mut buffer[done..], offset + done as u64)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "read ended early",
            ));
        }
        done += count;
    }
    Ok(())
}

fn parse_table(table: &[u8]) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for record in table.as_chunks::<VTBL_RECORD_SIZE>().0 {
        let length = u16::from_be_bytes([record[14], record[15]]) as usize;
        if length == 0 || length > VTBL_NAME_MAX {
            break;
        }
        let name = std::str::from_utf8(&record[VTBL_NAME_OFFSET..VTBL_NAME_OFFSET + length])
            .map_err(|_| "UBI volume table holds a name that is not UTF-8".to_string())?;
        names.push(name.to_string());
    }
    if names.is_empty() {
        return Err("UBI volume table is empty".to_string());
    }
    Ok(names)
}

pub fn volume_names(path: &Path, size: u64, erase_size: u64) -> Result<Vec<String>, String> {
    if erase_size < EC_HDR_SIZE as u64 + VID_HDR_SIZE as u64 {
        return Err("UBI partition reports an unusable eraseblock size".to_string());
    }
    let file = File::open(path)
        .map_err(|error| format!("Opening {} read-only failed: {error}", path.display()))?;

    let mut ec = [0u8; EC_HDR_SIZE];
    let mut vid = [0u8; VID_HDR_SIZE];
    for peb in 0..size / erase_size {
        let base = peb * erase_size;
        if read_exact_at(&file, base, &mut ec).is_err() || ec[..4] != EC_HDR_MAGIC {
            continue;
        }
        let Some(vid_offset) = be_u32(&ec, 16) else {
            continue;
        };
        let Some(data_offset) = be_u32(&ec, 20) else {
            continue;
        };
        if read_exact_at(&file, base + u64::from(vid_offset), &mut vid).is_err()
            || vid[..4] != VID_HDR_MAGIC
            || be_u32(&vid, 8) != Some(LAYOUT_VOLUME_ID)
        {
            continue;
        }

        let mut table = vec![0u8; VTBL_RECORD_SIZE * VTBL_RECORDS];
        read_exact_at(&file, base + u64::from(data_offset), &mut table)
            .map_err(|error| format!("Reading the UBI volume table failed: {error}"))?;
        return parse_table(&table);
    }

    Err(format!(
        "No UBI layout volume found in {} ({} eraseblocks scanned)",
        path.display(),
        size / erase_size
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_of(names: &[&str]) -> Vec<u8> {
        let mut table = vec![0u8; VTBL_RECORD_SIZE * VTBL_RECORDS];
        for (index, name) in names.iter().enumerate() {
            let record = &mut table[index * VTBL_RECORD_SIZE..(index + 1) * VTBL_RECORD_SIZE];
            record[14..16].copy_from_slice(&(name.len() as u16).to_be_bytes());
            record[VTBL_NAME_OFFSET..VTBL_NAME_OFFSET + name.len()]
                .copy_from_slice(name.as_bytes());
        }
        table
    }

    #[test]
    fn reads_names_in_volume_id_order() {
        let names = ["ubi_Config", "ubi_fpga", "ubi_DTB0"];
        assert_eq!(parse_table(&table_of(&names)).unwrap(), names);
    }

    #[test]
    fn stops_at_the_first_empty_record() {
        let table = table_of(&["ubi_Config", "ubi_DTB0"]);
        let mut later = table.clone();
        let record = &mut later[5 * VTBL_RECORD_SIZE..6 * VTBL_RECORD_SIZE];
        record[14..16].copy_from_slice(&4u16.to_be_bytes());
        record[VTBL_NAME_OFFSET..VTBL_NAME_OFFSET + 4].copy_from_slice(b"junk");
        assert_eq!(parse_table(&later).unwrap(), vec!["ubi_Config", "ubi_DTB0"]);
    }

    #[test]
    fn rejects_an_empty_table() {
        assert!(parse_table(&vec![0u8; VTBL_RECORD_SIZE * VTBL_RECORDS]).is_err());
    }
}
