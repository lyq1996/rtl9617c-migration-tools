// SPDX-License-Identifier: GPL-2.0-only

use std::time::{SystemTime, UNIX_EPOCH};

use crate::board::{self, Board};
use crate::fip::{BOOT_SIZE, ERASE_SIZE, FIP_OFFSETS, FIP_SIZE};
use crate::loader;
use crate::mtd::{self, MtdDevice};
use crate::ubi;

pub const BACKUP_PATH: &str = "/tmp/rtl9617c-boot-backup.bin";

const WRITE_SIZE: u64 = 0x800;

pub fn flash(upload: &[u8]) -> Result<String, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("Flashing requires root privileges".to_string());
    }

    let partitions = mtd::discover_partitions().map_err(|error| error.to_string())?;
    let board = resolve_board(&partitions);
    let partition = mtd::find_boot_partition(&partitions)?;
    let target = partition.path.display().to_string();
    let device =
        MtdDevice::open(partition).map_err(|error| format!("Opening {target} failed: {error}"))?;

    flash_into(&device, board, upload)
}

pub fn resolve_board(partitions: &[mtd::Partition]) -> Option<&'static Board> {
    let exposed: Vec<String> = partitions
        .iter()
        .filter(|partition| partition.is_ubi_volume())
        .map(|partition| partition.name.clone())
        .collect();
    if !exposed.is_empty() {
        return board::identify(&exposed).ok();
    }

    let partition = partitions
        .iter()
        .find(|partition| partition.name == "ubi_device")?;
    let volumes = ubi::volume_names(&partition.path, partition.size, partition.erase_size).ok()?;
    board::identify(&volumes).ok()
}

pub fn flash_into(
    device: &MtdDevice,
    board: Option<&Board>,
    upload: &[u8],
) -> Result<String, String> {
    let target = device.partition.path.display().to_string();
    if device.partition.size != BOOT_SIZE as u64
        || device.erase_size != ERASE_SIZE as u64
        || device.write_size != WRITE_SIZE
        || !device.is_nand()
    {
        return Err(format!(
            "{target} is not the expected NAND boot partition: size {}, eraseblock {}, \
             page {}, type {}",
            device.partition.size, device.erase_size, device.write_size, device.mtd_type
        ));
    }
    if !device.partition.writeable() {
        return Err(format!(
            "{target} is read-only (MTD_WRITEABLE is clear); unlock the MTD partitions first"
        ));
    }

    let mut current = vec![0u8; BOOT_SIZE];
    device
        .read_exact_at(0, &mut current)
        .map_err(|error| format!("Reading {target} failed: {error}"))?;

    let plan = loader::plan(&current, upload, board)?;

    let span = FIP_OFFSETS[1] + FIP_SIZE - FIP_OFFSETS[0];
    device.check_range_good(FIP_OFFSETS[0] as u64, span as u64)?;

    std::fs::write(BACKUP_PATH, &current).map_err(|error| {
        format!("Saving the current boot area to {BACKUP_PATH} failed: {error}")
    })?;

    let mut blocks = 0usize;
    for offset in FIP_OFFSETS.iter().rev() {
        for block in (*offset..offset + FIP_SIZE).step_by(ERASE_SIZE) {
            device
                .write_eraseblock(block as u64, &plan.image[block..block + ERASE_SIZE])
                .map_err(|error| {
                    format!(
                        "Writing {target} at 0x{block:x} failed: {error}. \
                         The previous boot area is in {BACKUP_PATH}"
                    )
                })?;
            blocks += 1;
        }
    }
    unsafe { libc::sync() };

    Ok(format!(
        "U-Boot {} bytes written to {target} at BL33 offset 0x{:x} \
         ({} eraseblocks, FIP1 then FIP0, readback verified, {} bytes spare); \
         previous boot area saved to {BACKUP_PATH}; rebooting",
        plan.bl33_size,
        plan.bl33_offset,
        blocks,
        plan.headroom()
    ))
}

pub fn backup_filename() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("boot-backup-{stamp}.bin")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partition(index: u32, name: &str) -> mtd::Partition {
        mtd::Partition {
            index,
            name: name.to_string(),
            size: 0x20_0000,
            erase_size: 0x2_0000,
            offset: Some(0),
            flags: None,
            path: std::path::PathBuf::from(format!("/dev/mtd{index}")),
        }
    }

    fn stock_table(board: &Board) -> Vec<mtd::Partition> {
        let mut partitions: Vec<_> = ["boot", "env", "env2", "static_conf", "ubi_device"]
            .iter()
            .enumerate()
            .map(|(index, name)| partition(index as u32, name))
            .collect();
        for (index, name) in board.volumes.iter().enumerate() {
            partitions.push(partition(5 + index as u32, name));
        }
        partitions
    }

    #[test]
    fn identifies_every_configured_board_from_the_stock_partition_list() {
        for board in board::all() {
            let identified = resolve_board(&stock_table(board)).unwrap();
            assert_eq!(identified.name, board.name);
        }
    }

    #[test]
    fn reports_no_board_when_only_physical_partitions_are_listed() {
        let partitions = vec![partition(0, "boot"), partition(1, "env")];
        assert!(resolve_board(&partitions).is_none());
    }
}
