// SPDX-License-Identifier: GPL-2.0-only
//! Dry run against a boot-partition backup file, no device involved.
//!
//!   MIGRATION_BOOT_BACKUP=/tmp/tyfza1-boot-stock.bin \
//!   MIGRATION_UPLOAD=.../output/tyfza1/bl33.bin \
//!   MIGRATION_BOARD=TYFZA1 \
//!   cargo test --test dry-run -- --nocapture

use rtl9617c_migration_tools::{board, fip, loader};

fn fixture() -> Option<(Vec<u8>, Vec<u8>, &'static board::Board)> {
    let current = std::fs::read(std::env::var("MIGRATION_BOOT_BACKUP").ok()?).ok()?;
    let upload = std::fs::read(std::env::var("MIGRATION_UPLOAD").ok()?).ok()?;
    let wanted = std::env::var("MIGRATION_BOARD").ok()?;
    let board = board::all()
        .iter()
        .find(|board| board.name == wanted)
        .unwrap_or_else(|| panic!("unknown board {wanted}"));
    assert_eq!(current.len(), fip::BOOT_SIZE, "backup must be the boot partition");
    Some((current, upload, board))
}

/// Prints how the tool handles the upload.
#[test]
fn probe_reports_the_plan_outcome() {
    let Some((current, upload, board)) = fixture() else {
        eprintln!("skip: MIGRATION_BOOT_BACKUP / MIGRATION_UPLOAD / MIGRATION_BOARD not set");
        return;
    };
    match loader::plan(&current, &upload, Some(board)) {
        Ok(plan) => println!(
            "ACCEPT: bl33 {} bytes at slot offset 0x{:x}, capacity {}, headroom {}",
            plan.bl33_size, plan.bl33_offset, plan.bl33_capacity, plan.headroom()
        ),
        Err(error) => println!("REJECT: {error}"),
    }
}

/// The full acceptance contract: the stub goes into BL33 and nothing else
/// moves - GPT, reserved tail, BL2, BL31, every certificate entry and the
/// ToC around them stay byte-for-byte, both copies agree, and the whole
/// image passes the device-preservation gates.
#[test]
fn stub_replaces_only_bl33_and_fits_the_slot() {
    let Some((current, upload, board)) = fixture() else {
        eprintln!("skip: MIGRATION_BOOT_BACKUP / MIGRATION_UPLOAD / MIGRATION_BOARD not set");
        return;
    };

    let plan = loader::plan(&current, &upload, Some(board))
        .expect("the stub must be accepted for its own board");
    assert_eq!(plan.bl33_size, upload.len());
    assert!(plan.bl33_size <= plan.bl33_capacity);
    let image = &plan.image;

    assert_eq!(&image[..0x4_0000], &current[..0x4_0000], "GPT area changed");
    assert_eq!(&image[0x1c_0000..], &current[0x1c_0000..], "reserved tail changed");

    for offset in fip::FIP_OFFSETS {
        let old = fip::parse(&current[offset..offset + fip::FIP_SIZE])
            .expect("the stock FIP must parse");
        let new = fip::parse(&image[offset..offset + fip::FIP_SIZE])
            .expect("the patched FIP must parse");
        assert_eq!(old.entries.len(), new.entries.len(), "entry count changed");

        for old_entry in &old.entries {
            let new_entry = new
                .payload(&old_entry.uuid)
                .unwrap_or_else(|| panic!("entry dropped: {:x?}", old_entry.uuid));
            assert_eq!(old_entry.offset, new_entry.offset, "entry offset moved");
            let old_dir = &current[offset + old_entry.entry_offset..][..40];
            let new_dir = &image[offset + new_entry.entry_offset..][..40];
            if old_entry.uuid == fip::UUID_BL33 {
                // Only the size dword of the BL33 directory entry may change.
                assert_eq!(&old_dir[..24], &new_dir[..24]);
                assert_eq!(&old_dir[32..], &new_dir[32..]);
            } else {
                assert_eq!(old_dir, new_dir, "non-BL33 directory entry changed");
                let old_payload = &current[offset + old_entry.offset..]
                    [..old_entry.size];
                let new_payload = &image[offset + new_entry.offset..][..new_entry.size];
                assert_eq!(old_payload, new_payload, "non-BL33 payload changed");
            }
        }

        let entry = new.payload(&fip::UUID_BL33).unwrap();
        assert_eq!(
            &image[offset + entry.offset..][..entry.size],
            &upload[..],
            "BL33 payload is not the uploaded stub"
        );
        let capacity = new.capacity(entry, fip::FIP_SIZE);
        assert!(
            image[offset + entry.offset + entry.size..offset + entry.offset + capacity]
                .iter()
                .all(|byte| *byte == 0),
            "the space up to the certificates must be zero-filled"
        );
    }

    assert_eq!(
        &image[0x4_0000..0x10_0000],
        &image[0x10_0000..0x1c_0000],
        "the two FIP copies diverge"
    );
    fip::preserves_device(image, &current)
        .expect("the patched image must pass the device-preservation gates");
}

/// A stub built for one board must not be accepted on the other.
#[test]
fn refuses_a_stub_from_the_other_board() {
    let Some((current, _upload, board)) = fixture() else {
        eprintln!("skip: MIGRATION_BOOT_BACKUP / MIGRATION_UPLOAD / MIGRATION_BOARD not set");
        return;
    };
    let other = board::all()
        .iter()
        .find(|other| other.name != board.name)
        .expect("a second board must be configured");
    let cross = std::env::var("MIGRATION_CROSS_UPLOAD").ok();
    let Some(upload) = cross else {
        eprintln!("skip: MIGRATION_CROSS_UPLOAD not set");
        return;
    };
    let upload = std::fs::read(upload).expect("cross upload must exist");
    let error = loader::plan(&current, &upload, Some(board)).unwrap_err();
    println!("cross-board rejection: {error}");
    assert!(!error.is_empty());
}
