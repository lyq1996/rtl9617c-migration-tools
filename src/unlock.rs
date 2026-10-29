// SPDX-License-Identifier: GPL-2.0-only

use std::ffi::CString;
use std::io;

use crate::embedded::MODULES;

const VERMAGIC_TAG: &[u8] = b"vermagic=";

/// The kernel series a release belongs to: `6.18.55` -> `6.18`, `6.18` -> `6.18`.
///
/// A module is ABI-stable within one series, so one build covers every
/// patch release of it; the exact release is stamped in at load time.
pub fn series(release: &str) -> &str {
    let mut parts = release.splitn(3, '.');
    let major = parts.next().unwrap_or(release);
    match parts.next() {
        Some(minor) => &release[..major.len() + 1 + minor.len()],
        None => release,
    }
}

pub fn bundled() -> Vec<&'static str> {
    MODULES.iter().map(|(series, _)| *series).collect()
}

/// Picks the module covering `release`'s series, together with that series.
pub fn module(release: &str) -> Option<(&'static str, &'static [u8])> {
    let wanted = series(release);
    MODULES
        .iter()
        .find(|(name, _)| *name == wanted)
        .map(|(name, bytes)| (*name, *bytes))
}

pub fn module_kernel_release(module: &[u8]) -> Option<&str> {
    let start = module
        .windows(VERMAGIC_TAG.len())
        .position(|window| window == VERMAGIC_TAG)?
        + VERMAGIC_TAG.len();
    let rest = &module[start..];
    let end = rest.iter().position(|byte| *byte == 0 || *byte == b' ')?;
    std::str::from_utf8(&rest[..end]).ok()
}

pub fn running_release() -> Result<String, String> {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map_err(|error| format!("Reading the kernel release failed: {error}"))?;
    Ok(release.trim().to_string())
}

/// Rewrites the release word of `image`'s vermagic to `release`, in place.
///
/// The kernel compares vermagic with a plain `strcmp` for a module without
/// symbol CRCs (the case for these builds), so the release has to match
/// byte for byte. Rewriting keeps the string length, which is why a release
/// of a different length cannot be patched in place.
fn retarget_vermagic(image: &mut [u8], release: &str, key: &str) -> Result<Option<String>, String> {
    let Some(start) = image
        .windows(VERMAGIC_TAG.len())
        .position(|window| window == VERMAGIC_TAG)
    else {
        return Err("The bundled module carries no vermagic".to_string());
    };
    let start = start + VERMAGIC_TAG.len();
    let rest = &image[start..];
    let end = match rest.iter().position(|byte| *byte == 0 || *byte == b' ') {
        Some(end) => end,
        None => return Err("The bundled module's vermagic is unterminated".to_string()),
    };
    let Ok(recorded) = std::str::from_utf8(&rest[..end]) else {
        return Err("The bundled module's vermagic is not valid UTF-8".to_string());
    };
    let recorded = recorded.to_string();
    // Guard: never re-stamp across series, even when the lengths happen to match.
    if series(&recorded) != key {
        return Err(format!("The {key} module was built for {recorded}"));
    }
    if recorded == release {
        return Ok(None);
    }
    if recorded.len() != release.len() {
        return Err(format!(
            "The bundled module was built for {recorded}, which cannot be re-stamped \
             to {release} in place; add a module for the {release} series"
        ));
    }
    image[start..start + end].copy_from_slice(release.as_bytes());
    Ok(Some(recorded))
}

fn symbol_address(symbols: &str, wanted: &str) -> Result<u64, String> {
    let address = symbols
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let address = fields.next()?;
            fields.next()?;
            (fields.next()? == wanted).then_some(address)
        })
        .next()
        .ok_or_else(|| format!("{wanted} is absent from /proc/kallsyms"))?;
    let address =
        u64::from_str_radix(address, 16).map_err(|_| format!("Invalid address for {wanted}"))?;
    if address == 0 {
        return Err(format!("Address of {wanted} is hidden by the kernel"));
    }
    Ok(address)
}

fn hint(error: &io::Error) -> Option<&'static str> {
    match error.raw_os_error()? {
        libc::ENOEXEC => {
            Some("the module ABI does not match this kernel; dmesg has the loader's reason")
        }
        libc::ENOENT => Some("the kernel does not export a symbol the module needs"),
        libc::EPERM => Some("module loading is disabled by the kernel"),
        libc::EKEYREJECTED => Some("the kernel requires signed modules"),
        _ => None,
    }
}

pub fn load(release: &str) -> Result<(), String> {
    let Some((key, module)) = module(release) else {
        return Err(format!(
            "No MTD module is bundled for kernel {release} (series {}); this build carries {}",
            series(release),
            bundled().join(", ")
        ));
    };
    let mut image = module.to_vec();
    if let Some(recorded) = retarget_vermagic(&mut image, release, key)? {
        println!("re-stamped the {key} module from {recorded} to {release}");
    }

    let symbols = std::fs::read_to_string("/proc/kallsyms")
        .map_err(|error| format!("Reading /proc/kallsyms failed: {error}"))?;
    let get = symbol_address(&symbols, "get_mtd_device")?;
    let put = symbol_address(&symbols, "put_mtd_device")?;

    let parameters = CString::new(format!(
        "get_mtd_device_addr=0x{get:x} put_mtd_device_addr=0x{put:x}"
    ))
    .unwrap();
    let result = unsafe {
        libc::syscall(
            libc::SYS_init_module,
            image.as_ptr(),
            image.len(),
            parameters.as_ptr(),
        )
    };
    if result == 0 {
        return Ok(());
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EEXIST) {
        return Ok(());
    }
    match hint(&error) {
        Some(hint) => Err(format!("init_module failed: {error} ({hint})")),
        None => Err(format!("init_module failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_module_carries_its_own_series() {
        assert!(!MODULES.is_empty(), "no modules were embedded");
        for (key, module) in MODULES {
            assert!(module.len() > 1024, "{key}: module looks truncated");
            let recorded = module_kernel_release(module)
                .unwrap_or_else(|| panic!("modules/{key}.ko carries no vermagic"));
            assert_eq!(
                series(recorded),
                *key,
                "modules/{key}.ko was built for {recorded}"
            );
        }
    }

    #[test]
    fn selects_the_module_covering_a_release() {
        for (key, _) in MODULES {
            for suffix in [".0", ".99"] {
                let release = format!("{key}{suffix}");
                let Some((selected, _)) = module(&release) else {
                    panic!("{release} selected nothing")
                };
                assert_eq!(selected, *key, "{release} selected {selected}");
            }
        }
        assert!(module("0.0.0-not-a-release").is_none());
    }

    #[test]
    fn series_strips_the_patch_level() {
        assert_eq!(series("6.18.55"), "6.18");
        assert_eq!(series("6.18"), "6.18");
        assert_eq!(series("5.10.70"), "5.10");
        assert_eq!(series("6"), "6");
    }

    #[test]
    fn vermagic_is_re_stamped_to_the_running_release() {
        let mut image = b"\x00vermagic=6.18.55 SMP mod_unload aarch64\x00".to_vec();
        let offset = image
            .windows(VERMAGIC_TAG.len())
            .position(|window| window == VERMAGIC_TAG)
            .unwrap()
            + VERMAGIC_TAG.len();

        assert_eq!(retarget_vermagic(&mut image, "6.18.55", "6.18").unwrap(), None);
        let rewritten = retarget_vermagic(&mut image, "6.18.59", "6.18").unwrap();
        assert_eq!(rewritten, Some("6.18.55".to_string()));
        assert_eq!(&image[offset..offset + 7], b"6.18.59");
        assert_eq!(
            module_kernel_release(&image),
            Some("6.18.59"),
            "the rewritten image must read back as the running release"
        );

        // A different length cannot be patched in place; refuse it instead.
        let error = retarget_vermagic(&mut image, "6.18.5", "6.18").unwrap_err();
        assert!(error.contains("cannot be re-stamped"), "{error}");
        assert_eq!(
            &image[offset..offset + 7],
            b"6.18.59",
            "a refusal must not corrupt the image"
        );
    }

    #[test]
    fn vermagic_is_never_re_stamped_across_series() {
        let mut image = b"\x00vermagic=5.10.70 SMP mod_unload aarch64\x00".to_vec();
        // Same length as 6.18.55, but a different series: refuse regardless.
        let error = retarget_vermagic(&mut image, "6.18.55", "6.18").unwrap_err();
        assert!(error.contains("built for 5.10.70"), "{error}");
        assert_eq!(module_kernel_release(&image), Some("5.10.70"));
    }

    #[test]
    fn reports_the_running_release() {
        let release = running_release().expect("the kernel release is readable");
        assert!(!release.is_empty());
        assert!(!release.contains(' '));
    }
}
