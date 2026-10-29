// SPDX-License-Identifier: GPL-2.0-only

use crate::board;
use crate::fip::{self, BOOT_SIZE, FIP_OFFSETS, FIP_SIZE, UUID_BL33, UUID_TB_FW};

#[derive(Debug, Clone)]
pub struct Plan {
    pub image: Vec<u8>,
    pub bl33_offset: usize,
    pub bl33_size: usize,
    pub bl33_capacity: usize,
    pub board: Option<&'static board::Board>,
    pub unknown_variant: bool,
    pub warning: String,
}

impl Plan {
    pub fn headroom(&self) -> usize {
        self.bl33_capacity.saturating_sub(self.bl33_size)
    }

    pub fn board_label(&self) -> String {
        match self.board {
            Some(unit) => unit.name.clone(),
            None => "unknown variant".to_string(),
        }
    }
}

pub fn plan(current: &[u8], upload: &[u8]) -> Result<Plan, String> {
    if current.len() != BOOT_SIZE {
        return Err(format!(
            "The boot partition is {} bytes; this platform uses {BOOT_SIZE}",
            current.len()
        ));
    }
    if upload.len() != BOOT_SIZE {
        return Err(format!(
            "The upload is {} bytes. Upload a complete {BOOT_SIZE}-byte boot image instead",
            upload.len()
        ));
    }

    let parsed = fip::parse_boot_image(upload)?;
    // The install writes each slot from this image; two diverging slots
    // would leave the fallback slot unverified, so require byte equality.
    let first = FIP_OFFSETS[0];
    for offset in FIP_OFFSETS {
        if upload[first..first + FIP_SIZE] != upload[offset..offset + FIP_SIZE] {
            return Err("The two FIP slots of the upload disagree".to_string());
        }
    }

    fip::preserves_device(upload, current)?;

    let entry = parsed[0].require(&UUID_BL33, "BL33/U-Boot")?.clone();
    let bl2 = parsed[0].require(&UUID_TB_FW, "BL2")?;
    let bl2_digest =
        fip::sha256(&upload[FIP_OFFSETS[0] + bl2.offset..FIP_OFFSETS[0] + bl2.offset + bl2.size]);
    let board = board::identify(&bl2_digest);
    let unknown_variant = board.is_none();
    let warning = if unknown_variant {
        let prefix: String = bl2_digest[..4]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!(
            "The image matches this device's BL2 and BL31, but this device's \
             BL2 fingerprint {prefix}… is not in the known board table, so the \
             board model cannot be confirmed"
        )
    } else {
        String::new()
    };

    Ok(Plan {
        bl33_offset: entry.offset,
        bl33_size: entry.size,
        bl33_capacity: parsed[0].capacity(&entry, FIP_SIZE),
        image: upload.to_vec(),
        board,
        unknown_variant,
        warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board;
    use crate::fip::fixture::{arm64, boot_image};

    fn board(name: &str) -> &'static board::Board {
        board::all()
            .iter()
            .find(|board| board.name == name)
            .expect("board is configured")
    }

    fn u_boot(len: usize, unit: &board::Board) -> Vec<u8> {
        let tree = crate::dtb::fixture::blob(&unit.name, &[&unit.compatible, "realtek,rtl9617c"]);
        assert!(len > tree.len() + 8);
        let mut data = vec![0x33u8; len];
        data[..4].copy_from_slice(&0x1400_000au32.to_le_bytes());
        let start = (len - tree.len()) & !7;
        data[start..start + tree.len()].copy_from_slice(&tree);
        data
    }

    fn stock(unit: &board::Board) -> Vec<u8> {
        let (bl2, bl31) = match unit.name.as_str() {
            "PT1500-H10" => (0x20680, 0x1e4d8),
            _ => (0x20718, 0x1e5a0),
        };
        boot_image(
            &arm64(bl2, 0x11),
            &arm64(bl31, 0x22),
            &u_boot(0x900, unit),
            6,
        )
    }

    #[test]
    fn accepts_the_devices_own_image() {
        let unit = board("TYFZA1");
        let current = stock(unit);
        let plan = plan(&current, &current).unwrap();
        assert_eq!(plan.image, current);
    }

    #[test]
    fn flags_a_matching_image_from_an_unknown_board() {
        let unit = board("TYFZA1");
        let mut current = stock(unit);
        current[0x40_200] ^= 0xff;
        current[0x100_200] ^= 0xff;
        let outcome = plan(&current, &current).unwrap();
        assert!(outcome.unknown_variant);
        assert!(outcome.board.is_none());
        assert!(
            outcome.warning.contains("not in the known board table"),
            "{}",
            outcome.warning
        );
        let mismatched = stock(unit);
        let error = plan(&current, &mismatched).unwrap_err();
        assert!(error.contains("another board"), "{error}");
    }

    #[test]
    fn rejects_uploads_that_are_not_full_boot_images() {
        let unit = board("TYFZA1");
        let current = stock(unit);
        for upload in [&[][..], &[0u8; 0x100][..], u_boot(0x500, unit).as_slice()] {
            let error = plan(&current, upload).unwrap_err();
            assert!(error.contains("complete"), "{error}");
        }
    }

    #[test]
    fn accepts_an_image_with_rebuilt_certificates_and_a_new_payload() {
        let unit = board("TYFZA1");
        let current = stock(unit);
        let replacement = arm64(0x800, 0x44);
        let mut upload = current.clone();
        {
            let slot = FIP_OFFSETS[0];
            let fip = fip::parse(&current[slot..slot + FIP_SIZE]).unwrap();
            let bl33 = fip.require(&UUID_BL33, "BL33").unwrap();
            // Overwrite the BL33 payload and every certificate byte: the
            // shape a re-signed chain produces.
            upload[slot + bl33.offset..slot + bl33.offset + replacement.len()]
                .copy_from_slice(&replacement);
            upload[slot + bl33.offset + bl33.size..slot + bl33.offset + bl33.size + 24].fill(0x77);
            let directory = slot + bl33.entry_offset + 24..slot + bl33.entry_offset + 32;
            upload[directory.start..directory.end]
                .copy_from_slice(&(replacement.len() as u64).to_le_bytes());
            let slot_bytes = upload[slot..slot + FIP_SIZE].to_vec();
            for offset in FIP_OFFSETS {
                upload[offset..offset + FIP_SIZE].copy_from_slice(&slot_bytes);
            }
        }
        let plan = plan(&current, &upload).unwrap();
        fip::preserves_device(&plan.image, &current).unwrap();
    }

    #[test]
    fn refuses_a_complete_image_built_for_another_board() {
        let unit = board("PT1500-H10");
        let other = stock(board("TYFZA1"));
        let error = plan(&stock(unit), &other).unwrap_err();
        assert!(error.contains("another board"), "{error}");
    }

    #[test]
    fn refuses_slots_that_disagree_outside_the_payload_checks() {
        let unit = board("TYFZA1");
        let mut upload = stock(unit);
        upload[FIP_OFFSETS[1] + 0xb1000] ^= 0xff;
        let error = plan(&stock(unit), &upload).unwrap_err();
        assert!(error.contains("disagree"), "{error}");
    }

    #[test]
    fn rejects_a_wrong_sized_device() {
        let unit = board("TYFZA1");
        let error = plan(&stock(unit)[..0x1000], &stock(unit)).unwrap_err();
        assert!(error.contains("boot partition is"), "{error}");
    }
}
