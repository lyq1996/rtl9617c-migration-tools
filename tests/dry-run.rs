// SPDX-License-Identifier: GPL-2.0-only
//! Dry run against a boot-partition backup file, no device involved.
//!
//! Defaults to the files in `tests/fixtures/`:
//!
//!   cargo test --test dry-run -- --nocapture
//!
//! `MIGRATION_BOOT_BACKUP` / `MIGRATION_UPLOAD` override the defaults.

use rtl9617c_migration_tools::{fip, loader};

/// Env var wins; otherwise fall back to tests/fixtures/<fallback>.
fn read_input(env: &str, fallback: &str) -> Option<Vec<u8>> {
    if let Ok(path) = std::env::var(env) {
        return Some(
            std::fs::read(&path).unwrap_or_else(|error| panic!("read {path} from {env}: {error}")),
        );
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fallback);
    std::fs::read(&path).ok()
}

fn fixture() -> Option<(Vec<u8>, Vec<u8>)> {
    let current = read_input("MIGRATION_BOOT_BACKUP", "stock-mtd0.bin")?;
    let upload = read_input("MIGRATION_UPLOAD", "boot.bin")?;
    assert_eq!(
        current.len(),
        fip::BOOT_SIZE,
        "backup must be the boot partition"
    );
    Some((current, upload))
}

/// Prints how the tool handles the upload.
#[test]
fn probe_reports_the_plan_outcome() {
    let Some((current, upload)) = fixture() else {
        eprintln!("skip: tests/fixtures/stock-mtd0.bin + boot.bin not present");
        return;
    };
    match loader::plan(&current, &upload) {
        Ok(plan) => println!(
            "ACCEPT: bl33 {} bytes at slot offset 0x{:x}, capacity {}, headroom {}",
            plan.bl33_size,
            plan.bl33_offset,
            plan.bl33_capacity,
            plan.headroom()
        ),
        Err(error) => println!("REJECT: {error}"),
    }
}

#[test]
fn full_image_preserves_the_device() {
    let Some((current, upload)) = fixture() else {
        eprintln!("skip: tests/fixtures/stock-mtd0.bin + boot.bin not present");
        return;
    };

    let plan = loader::plan(&current, &upload).expect("the packaged boot image must be accepted");
    let image = &plan.image;

    assert_eq!(&image[..0x4_0000], &current[..0x4_0000], "GPT area changed");
    assert_eq!(
        &image[0x1c_0000..],
        &current[0x1c_0000..],
        "reserved tail changed"
    );

    for offset in fip::FIP_OFFSETS {
        let old =
            fip::parse(&current[offset..offset + fip::FIP_SIZE]).expect("the stock FIP must parse");
        let new =
            fip::parse(&image[offset..offset + fip::FIP_SIZE]).expect("the new FIP must parse");
        assert_eq!(old.entries.len(), new.entries.len(), "entry count changed");

        for role in [fip::UUID_TB_FW, fip::UUID_BL31] {
            let old_entry = old.payload(&role).unwrap();
            let new_entry = new.payload(&role).unwrap();
            assert_eq!(
                old_entry.size, new_entry.size,
                "pinned payload size changed"
            );
            assert_eq!(
                &current[offset + old_entry.offset..][..old_entry.size],
                &image[offset + new_entry.offset..][..new_entry.size],
                "pinned payload changed"
            );
        }
    }

    assert_eq!(
        &image[0x4_0000..0x10_0000],
        &image[0x10_0000..0x1c_0000],
        "the two FIP copies diverge"
    );
    fip::preserves_device(image, &current)
        .expect("the packaged image must pass the device-preservation gates");
}

/// A bare U-Boot upload is refused with guidance, not silently patched.
#[test]
fn bare_u_boot_uploads_are_refused() {
    let Some((current, _)) = fixture() else {
        eprintln!("skip: tests/fixtures/stock-mtd0.bin + boot.bin not present");
        return;
    };
    let small = vec![0u8; 0x500];
    let error = loader::plan(&current, &small).unwrap_err();
    assert!(error.contains("complete"), "{error}");
}
