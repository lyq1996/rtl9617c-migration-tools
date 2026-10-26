// SPDX-License-Identifier: GPL-2.0-only

use crate::board::Board;
use crate::dtb;
use crate::fip::{self, BOOT_SIZE, FIP_OFFSETS, FIP_SIZE, Fip, UUID_BL33};

#[derive(Debug, Clone)]
pub struct Plan {
    pub image: Vec<u8>,
    pub bl33_offset: usize,
    pub bl33_size: usize,
    pub bl33_capacity: usize,
}

impl Plan {
    pub fn headroom(&self) -> usize {
        self.bl33_capacity.saturating_sub(self.bl33_size)
    }
}

fn looks_like_arm64_image(data: &[u8]) -> bool {
    let Some(word) = data.get(..4) else {
        return false;
    };
    let opcode = u32::from_le_bytes(word.try_into().unwrap()) >> 26;
    opcode == 0b000101 || opcode == 0b100101
}

fn bl33_payload(image: &[u8]) -> Result<&[u8], String> {
    let fip = fip::parse(&image[FIP_OFFSETS[0]..FIP_OFFSETS[0] + FIP_SIZE])?;
    let entry = fip.require(&UUID_BL33, "BL33/U-Boot")?;
    Ok(&image[FIP_OFFSETS[0] + entry.offset..][..entry.size])
}

/// The LZMA stub embeds this prefix plus its board name (start.S,
/// .rodata.stub_id). A stub is not a plain u-boot.bin: no branch as the
/// first instruction, no readable device tree. The board name takes over
/// the compatible check.
const STUB_ID: &[u8] = b"RTL9-BL33-LZMA-STUB:";

fn stub_board(upload: &[u8]) -> Option<String> {
    let start = upload
        .windows(STUB_ID.len())
        .position(|window| window == STUB_ID)?;
    let rest = &upload[start + STUB_ID.len()..];
    // .asciz puts a NUL between the two strings; accept both layouts.
    let rest = if rest.first() == Some(&0) { &rest[1..] } else { rest };
    let end = rest.iter().position(|byte| *byte == 0)?;
    if end == 0 {
        return None;
    }
    String::from_utf8(rest[..end].to_vec()).ok()
}

fn check_board(image: &[u8], board: &Board) -> Result<(), String> {
    let payload = bl33_payload(image)?;
    let tree = dtb::find_appended(payload).ok_or_else(|| {
        "The uploaded U-Boot carries no appended device tree, so the board it was built for \
         cannot be checked"
            .to_string()
    })?;
    let compatible = dtb::compatible(tree).ok_or_else(|| {
        "The uploaded U-Boot's device tree has no compatible property, so the board it was \
         built for cannot be checked"
            .to_string()
    })?;
    if !compatible.contains(&board.compatible.as_str()) {
        let model = dtb::model(tree).unwrap_or("unnamed");
        return Err(format!(
            "The uploaded U-Boot is built for {} ({model}); this unit is a {}",
            compatible.join(", "),
            board.name
        ));
    }
    Ok(())
}

fn reference_fip(current: &[u8]) -> Result<Fip, String> {
    let mut last = String::new();
    for offset in FIP_OFFSETS {
        match fip::parse(&current[offset..offset + FIP_SIZE]) {
            Ok(fip) => return Ok(fip),
            Err(error) => last = error,
        }
    }
    Err(format!(
        "Neither FIP copy on this device is readable ({last}); upload a complete boot image instead"
    ))
}

fn patch_bl33(current: &[u8], bl33: &[u8]) -> Result<Vec<u8>, String> {
    if bl33.is_empty() {
        return Err("The uploaded U-Boot image is empty".to_string());
    }
    if stub_board(bl33).is_none() && !looks_like_arm64_image(bl33) {
        return Err(
            "The uploaded file does not start like an AArch64 U-Boot image; upload u-boot.bin"
                .to_string(),
        );
    }

    let fip = reference_fip(current)?;
    let entry = fip.require(&UUID_BL33, "BL33/U-Boot")?.clone();
    let capacity = fip.capacity(&entry, FIP_SIZE);
    if bl33.len() > capacity {
        return Err(format!(
            "U-Boot is {} bytes but the BL33 slot on this device holds {capacity}; \
             it must not grow into the certificate chain",
            bl33.len()
        ));
    }

    let mut slot = current[FIP_OFFSETS[0]..FIP_OFFSETS[0] + FIP_SIZE].to_vec();
    slot[entry.entry_offset + 24..entry.entry_offset + 32]
        .copy_from_slice(&(bl33.len() as u64).to_le_bytes());
    slot[entry.offset..entry.offset + bl33.len()].copy_from_slice(bl33);
    slot[entry.offset + bl33.len()..entry.offset + capacity].fill(0);

    let mut image = current.to_vec();
    for offset in FIP_OFFSETS {
        image[offset..offset + FIP_SIZE].copy_from_slice(&slot);
    }
    Ok(image)
}

pub fn plan(current: &[u8], upload: &[u8], board: Option<&Board>) -> Result<Plan, String> {
    if current.len() != BOOT_SIZE {
        return Err(format!(
            "The boot partition is {} bytes; this platform uses {BOOT_SIZE}",
            current.len()
        ));
    }

    let image = if upload.len() == BOOT_SIZE {
        fip::parse_boot_image(upload)?;
        upload.to_vec()
    } else {
        let Some(board) = board else {
            return Err(
                "This unit's UBI volume table could not be read, so a bare u-boot.bin cannot \
                 be checked against it; upload a complete boot image instead"
                    .to_string(),
            );
        };
        let stub = stub_board(upload);
        if let Some(name) = &stub {
            if *name != board.name {
                return Err(format!(
                    "The uploaded image is a BL33 stub for {name}; this unit is a {}",
                    board.name
                ));
            }
        }
        let image = patch_bl33(current, upload)?;
        if stub.is_none() {
            check_board(&image, board)?;
        }
        image
    };

    let parsed = fip::parse_boot_image(&image)?;
    fip::preserves_device(&image, current)?;

    let entry = parsed[0].require(&UUID_BL33, "BL33/U-Boot")?.clone();
    Ok(Plan {
        bl33_offset: entry.offset,
        bl33_size: entry.size,
        bl33_capacity: parsed[0].capacity(&entry, FIP_SIZE),
        image,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board;
    use crate::dtb::fixture::blob;
    use crate::fip::fixture::{arm64, boot_image};

    fn board(name: &str) -> &'static Board {
        board::all()
            .iter()
            .find(|board| board.name == name)
            .expect("board is configured")
    }

    fn u_boot(len: usize, board: &Board) -> Vec<u8> {
        let tree = blob(&board.name, &[&board.compatible, "realtek,rtl9617c"]);
        assert!(len > tree.len() + 8);
        let mut data = vec![0x33u8; len];
        data[..4].copy_from_slice(&0x1400_000au32.to_le_bytes());
        let start = (len - tree.len()) & !7;
        data[start..start + tree.len()].copy_from_slice(&tree);
        data
    }

    fn stock(board: &Board) -> Vec<u8> {
        let (bl2, bl31) = match board.name.as_str() {
            "PT1500-H10" => (0x20680, 0x1e4d8),
            _ => (0x20718, 0x1e5a0),
        };
        boot_image(
            &arm64(bl2, 0x11),
            &arm64(bl31, 0x22),
            &u_boot(0x900, board),
            6,
        )
    }

    #[test]
    fn patches_a_smaller_u_boot_into_the_devices_own_fip() {
        let unit = board("TYFZA1");
        let current = stock(unit);
        let upload = u_boot(0x500, unit);
        let plan = plan(&current, &upload, Some(unit)).unwrap();

        assert_eq!(plan.bl33_size, 0x500);
        assert_eq!(plan.bl33_capacity, 0x900);
        assert_eq!(plan.headroom(), 0x400);
        assert_eq!(&plan.image[..FIP_OFFSETS[0]], &current[..FIP_OFFSETS[0]]);
        assert_eq!(&plan.image[0x1c_0000..], &current[0x1c_0000..]);
        let first = &plan.image[FIP_OFFSETS[0] + plan.bl33_offset..][..0x500];
        let second = &plan.image[FIP_OFFSETS[1] + plan.bl33_offset..][..0x500];
        assert_eq!(first, second);
        assert_eq!(first, &upload[..]);
    }

    #[test]
    fn refuses_a_u_boot_built_for_another_board() {
        let unit = board("PT1500-H10");
        let upload = u_boot(0x500, board("TYFZA1"));
        let error = plan(&stock(unit), &upload, Some(unit)).unwrap_err();
        assert!(error.contains("tianyi,tyfza1"), "{error}");
    }

    #[test]
    fn refuses_a_u_boot_without_a_device_tree() {
        let unit = board("PT1500-H10");
        let error = plan(&stock(unit), &arm64(0x500, 0x44), Some(unit)).unwrap_err();
        assert!(error.contains("no appended device tree"), "{error}");
    }

    #[test]
    fn refuses_a_u_boot_that_does_not_fit_the_slot() {
        let unit = board("PT1500-H10");
        let upload = u_boot(0xa00, unit);
        assert!(plan(&stock(unit), &upload, Some(unit)).is_err());
    }

    #[test]
    fn refuses_a_file_that_is_not_an_arm64_image() {
        let unit = board("PT1500-H10");
        let current = stock(unit);
        assert!(plan(&current, &[0u8; 0x100], Some(unit)).is_err());
        assert!(plan(&current, &[], Some(unit)).is_err());
    }

    #[test]
    fn accepts_a_complete_boot_image_for_this_board() {
        let unit = board("TYFZA1");
        let current = stock(unit);
        assert_eq!(plan(&current, &current, Some(unit)).unwrap().image, current);
    }

    #[test]
    fn refuses_a_complete_image_built_for_another_board() {
        let unit = board("PT1500-H10");
        let other = stock(board("TYFZA1"));
        assert!(plan(&stock(unit), &other, Some(unit)).is_err());
    }

    #[test]
    fn rejects_an_upload_of_the_wrong_length() {
        let unit = board("PT1500-H10");
        let upload = u_boot(0x100, unit);
        assert!(plan(&stock(unit)[..0x1000], &upload, Some(unit)).is_err());
    }

    #[test]
    fn needs_the_board_only_for_a_bare_u_boot() {
        let unit = board("TYFZA1");
        let current = stock(unit);
        assert!(plan(&current, &current, None).is_ok());
        assert!(plan(&current, &u_boot(0x500, unit), None).is_err());
    }

    fn stub(name: &str, len: usize) -> Vec<u8> {
        // Shape of a real build-bl33.sh product: AArch64 code that does not
        // start with a branch, and the embedded ID string somewhere inside.
        let mut data = vec![0u8; len];
        data[..4].copy_from_slice(&0xaa00_03f3u32.to_le_bytes());
        let id = format!("RTL9-BL33-LZMA-STUB:\0{name}\0");
        data[0x100..0x100 + id.len()].copy_from_slice(id.as_bytes());
        data
    }

    #[test]
    fn accepts_the_lzma_stub_for_this_board() {
        let unit = board("TYFZA1");
        let current = stock(unit);
        let upload = stub("TYFZA1", 0x500);
        let plan = plan(&current, &upload, Some(unit)).unwrap();
        assert_eq!(plan.bl33_size, 0x500);
        assert!(plan.headroom() > 0);
        fip::preserves_device(&plan.image, &current).unwrap();
    }

    #[test]
    fn refuses_the_lzma_stub_of_another_board() {
        let unit = board("PT1500-H10");
        let upload = stub("TYFZA1", 0x500);
        let error = plan(&stock(unit), &upload, Some(unit)).unwrap_err();
        assert!(error.contains("TYFZA1") && error.contains("PT1500-H10"), "{error}");
    }

    #[test]
    fn refuses_an_oversized_stub() {
        let unit = board("TYFZA1");
        let upload = stub("TYFZA1", 0xa00);
        let error = plan(&stock(unit), &upload, Some(unit)).unwrap_err();
        assert!(error.contains("certificate chain"), "{error}");
    }

    #[test]
    fn still_checks_a_bare_u_boot_without_a_stub_id() {
        let unit = board("TYFZA1");
        let current = stock(unit);
        assert!(plan(&current, &arm64(0x500, 0x44), Some(unit)).is_err());
    }
}
