// SPDX-License-Identifier: GPL-2.0-only

use crate::display;
use crate::fip::{self, BOOT_SIZE, ERASE_SIZE, FIP_OFFSETS, FIP_SIZE};
use crate::loader;
use crate::mtd::{self, MtdDevice};
use std::os::unix::fs::FileExt;

const WRITE_SIZE: u64 = 0x800;

pub struct FlashError {
    pub code: Option<&'static str>,
    pub message: String,
}

impl FlashError {
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }
}

pub struct DeviceIdentity {
    pub text: String,
    pub known: bool,
}

fn root_error() -> FlashError {
    FlashError::fatal("This operation requires root privileges")
}

pub fn flash(upload: &[u8], override_unknown: bool) -> Result<String, FlashError> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(root_error());
    }

    let partitions =
        mtd::discover_partitions().map_err(|error| FlashError::fatal(error.to_string()))?;
    let partition = mtd::find_boot_partition(&partitions).map_err(FlashError::fatal)?;
    let target = partition.path.display().to_string();
    let device = MtdDevice::open(partition)
        .map_err(|error| FlashError::fatal(format!("Opening {target} failed: {error}")))?;

    flash_into(&device, upload, override_unknown)
}

pub fn clear_env() -> Result<String, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("Clearing the environment requires root privileges".to_string());
    }

    let partitions = mtd::discover_partitions().map_err(|error| error.to_string())?;
    let mut erased = Vec::new();
    for name in ["env", "env2"] {
        let Some(partition) = partitions.iter().find(|partition| partition.name == name) else {
            continue;
        };
        if !partition.writeable() {
            return Err(format!(
                "{} is read-only; unlock the MTD partitions before clearing the environment",
                partition.path.display()
            ));
        }
        let device = MtdDevice::open(partition.clone())
            .map_err(|error| format!("Opening {} failed: {error}", partition.path.display()))?;
        device
            .erase_partition()
            .map_err(|error| format!("Erasing {} failed: {error}", partition.path.display()))?;
        erased.push(format!(
            "{} ({})",
            partition.name,
            display::format_size(partition.size)
        ));
    }

    if erased.is_empty() {
        return Err("No env or env2 partition exists on this device".to_string());
    }
    unsafe { libc::sync() };
    Ok(format!("Erased {}", erased.join(" and ")))
}

pub fn identify_device(partitions: &[mtd::Partition]) -> DeviceIdentity {
    let fail = |detail: String| DeviceIdentity {
        text: detail,
        known: false,
    };
    let Ok(partition) = mtd::find_boot_partition(partitions) else {
        return fail("no boot partition found".to_string());
    };
    let file = match std::fs::File::open(&partition.path) {
        Ok(file) => file,
        Err(error) => {
            return fail(format!(
                "cannot read {} ({error})",
                partition.path.display()
            ));
        }
    };
    let mut image = vec![0u8; BOOT_SIZE];
    if let Err(error) = file.read_exact_at(&mut image, 0) {
        return fail(format!(
            "cannot read {} ({error})",
            partition.path.display()
        ));
    }
    let parsed = match fip::parse(&image[FIP_OFFSETS[0]..FIP_OFFSETS[0] + FIP_SIZE]) {
        Ok(parsed) => parsed,
        Err(error) => return fail(format!("boot partition is not a valid FIP image ({error})")),
    };
    let bl2 = match parsed.require(&fip::UUID_TB_FW, "BL2") {
        Ok(entry) => entry,
        Err(error) => return fail(format!("boot partition is not a valid FIP image ({error})")),
    };
    let digest =
        fip::sha256(&image[FIP_OFFSETS[0] + bl2.offset..FIP_OFFSETS[0] + bl2.offset + bl2.size]);
    let prefix: String = digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    match crate::board::identify(&digest) {
        Some(board) => DeviceIdentity {
            text: format!("{} ({})", board.name, board.compatible),
            known: true,
        },
        None => DeviceIdentity {
            text: format!("unknown variant (BL2 {prefix}…)"),
            known: false,
        },
    }
}

pub fn flash_into(
    device: &MtdDevice,
    upload: &[u8],
    override_unknown: bool,
) -> Result<String, FlashError> {
    let target = device.partition.path.display().to_string();
    if device.partition.size != BOOT_SIZE as u64
        || device.erase_size != ERASE_SIZE as u64
        || device.write_size != WRITE_SIZE
        || !device.is_nand()
    {
        return Err(FlashError::fatal(format!(
            "{target} is not the expected NAND boot partition: size {}, eraseblock {}, \
             write size {}, type {}",
            device.partition.size, device.erase_size, device.write_size, device.mtd_type
        )));
    }
    if !device.partition.writeable() {
        return Err(FlashError::fatal(format!(
            "{target} is read-only (MTD_WRITEABLE is clear); unlock the MTD partitions first"
        )));
    }

    let mut current = vec![0u8; BOOT_SIZE];
    device
        .read_exact_at(0, &mut current)
        .map_err(|error| FlashError::fatal(format!("Reading {target} failed: {error}")))?;

    let plan = loader::plan(&current, upload).map_err(FlashError::fatal)?;
    if plan.unknown_variant && !override_unknown {
        return Err(FlashError {
            code: Some("unknown-variant"),
            message: plan.warning.clone(),
        });
    }

    let span = FIP_OFFSETS[1] + FIP_SIZE - FIP_OFFSETS[0];
    device
        .check_range_good(FIP_OFFSETS[0] as u64, span as u64)
        .map_err(FlashError::fatal)?;

    let mut blocks = 0usize;
    for offset in FIP_OFFSETS.iter().rev() {
        for block in (*offset..offset + FIP_SIZE).step_by(ERASE_SIZE) {
            device
                .write_eraseblock(block as u64, &plan.image[block..block + ERASE_SIZE])
                .map_err(|error| {
                    FlashError::fatal(format!("Writing {target} at 0x{block:x} failed: {error}"))
                })?;
            blocks += 1;
        }
    }
    unsafe { libc::sync() };

    Ok(format!(
        "Both FIP slots rewritten in {target} \
         ({} eraseblocks, standby first, readback verified); \
         U-Boot {} bytes, {} bytes headroom",
        blocks,
        plan.bl33_size,
        plan.headroom()
    ))
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

    #[test]
    fn reports_no_board_when_the_boot_partition_is_missing() {
        let partitions = vec![partition(1, "env")];
        assert_eq!(identify_device(&partitions).text, "no boot partition found");
    }
}
