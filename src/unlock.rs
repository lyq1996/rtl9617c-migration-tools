// SPDX-License-Identifier: GPL-2.0-only

use std::ffi::CString;
use std::io;

use crate::embedded::MODULES;

const VERMAGIC_TAG: &[u8] = b"vermagic=";

pub fn bundled() -> Vec<String> {
    MODULES
        .iter()
        .map(|(series, variant, _)| format!("{series}-{variant}"))
        .collect()
}

pub fn kernel_series(release: &str) -> Option<&str> {
    let mut parts = release.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    if !major.chars().all(|c| c.is_ascii_digit()) || !minor.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(&release[..major.len() + 1 + minor.len()])
}

pub fn modules(series: &str) -> Vec<(&'static str, &'static [u8])> {
    MODULES
        .iter()
        .filter(|(candidate, _, _)| *candidate == series)
        .map(|(_, variant, bytes)| (*variant, *bytes))
        .collect()
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

fn patch_vermagic(module: &[u8], release: &str) -> Result<Vec<u8>, String> {
    let tag = module
        .windows(VERMAGIC_TAG.len())
        .position(|window| window == VERMAGIC_TAG)
        .ok_or_else(|| "the module carries no vermagic".to_string())?;
    let start = tag + VERMAGIC_TAG.len();
    if module[start..]
        .windows(VERMAGIC_TAG.len())
        .any(|window| window == VERMAGIC_TAG)
    {
        return Err("the module carries more than one vermagic".to_string());
    }
    let end = start
        + module[start..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| "the vermagic is not terminated".to_string())?;
    let value = &module[start..end];
    let space = value
        .iter()
        .position(|byte| *byte == b' ')
        .ok_or_else(|| "the vermagic carries no flags".to_string())?;
    let flags = &value[space + 1..];
    let new_len = release.len() + 1 + flags.len();
    if new_len > value.len() {
        return Err(format!(
            "the module vermagic has room for {} bytes, {release} needs {new_len}",
            value.len()
        ));
    }
    let mut patched = module.to_vec();
    patched[start..start + release.len()].copy_from_slice(release.as_bytes());
    patched[start + release.len()] = b' ';
    patched[start + release.len() + 1..start + new_len].copy_from_slice(flags);
    patched[start + new_len..end].fill(0);
    Ok(patched)
}

pub fn running_release() -> Result<String, String> {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map_err(|error| format!("Reading the kernel release failed: {error}"))?;
    Ok(release.trim().to_string())
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
        libc::ENOEXEC => Some("the module ABI does not match this kernel"),
        libc::ENOENT => Some("the kernel does not export a symbol the module needs"),
        libc::EPERM => Some("module loading is disabled by the kernel"),
        libc::EKEYREJECTED => Some("the kernel requires signed modules"),
        _ => None,
    }
}

pub fn load(release: &str) -> Result<(), String> {
    let series =
        kernel_series(release).ok_or_else(|| format!("Unrecognized kernel release: {release}"))?;
    let candidates = modules(series);
    if candidates.is_empty() {
        return Err(format!(
            "No MTD module is bundled for kernel series {series}; this build carries {}",
            bundled().join(", ")
        ));
    }

    let symbols = std::fs::read_to_string("/proc/kallsyms")
        .map_err(|error| format!("Reading /proc/kallsyms failed: {error}"))?;
    let get = symbol_address(&symbols, "get_mtd_device")?;
    let put = symbol_address(&symbols, "put_mtd_device")?;

    let parameters = CString::new(format!(
        "get_mtd_device_addr=0x{get:x} put_mtd_device_addr=0x{put:x}"
    ))
    .unwrap();

    let mut failures = Vec::new();
    for (variant, module) in candidates {
        let Some(recorded) = module_kernel_release(module) else {
            failures.push(format!("{series}-{variant}: no vermagic"));
            continue;
        };
        if kernel_series(recorded) != Some(series) {
            failures.push(format!("{series}-{variant}: vermagic {recorded}"));
            continue;
        }
        let patched = match patch_vermagic(module, release) {
            Ok(patched) => patched,
            Err(error) => {
                failures.push(format!("{series}-{variant}: {error}"));
                continue;
            }
        };
        let result = unsafe {
            libc::syscall(
                libc::SYS_init_module,
                patched.as_ptr(),
                patched.len(),
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
        if error.raw_os_error() == Some(libc::ENOEXEC) {
            failures.push(format!("{series}-{variant}: {error}"));
            continue;
        }
        return Err(match hint(&error) {
            Some(hint) => format!("init_module failed for {series}-{variant}: {error} ({hint})"),
            None => format!("init_module failed for {series}-{variant}: {error}"),
        });
    }

    Err(format!(
        "No bundled MTD module for {series} matched kernel {release}: {}",
        failures.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_series_from_a_release() {
        assert_eq!(kernel_series("6.18.63"), Some("6.18"));
        assert_eq!(kernel_series("5.10.274"), Some("5.10"));
        assert_eq!(kernel_series("6.18"), Some("6.18"));
        assert_eq!(kernel_series("6.x.1"), None);
        assert_eq!(kernel_series("6"), None);
    }

    #[test]
    fn patches_the_release_and_keeps_the_flags() {
        let module =
            b"x\0vermagic=6.18.63-rtl9617c-module-slot-0123456789abcdef SMP mod_unload aarch64\0y";
        let patched = patch_vermagic(module, "6.18.55").expect("the release fits");
        assert_eq!(patched.len(), module.len());
        assert_eq!(
            &patched[2..2 + b"vermagic=6.18.55 SMP mod_unload aarch64\0".len()],
            b"vermagic=6.18.55 SMP mod_unload aarch64\0"
        );
    }

    #[test]
    fn rejects_a_release_that_is_too_long() {
        let module = b"vermagic=6.0 SMP mod_unload aarch64\0";
        assert!(patch_vermagic(module, "6.18.12345").is_err());
    }

    #[test]
    fn rejects_a_module_without_vermagic() {
        assert!(patch_vermagic(b"no magic here\0", "6.18.55").is_err());
    }

    #[test]
    fn every_bundled_module_belongs_to_its_kernel_series() {
        if MODULES.is_empty() {
            return;
        }
        for (series, variant, module) in MODULES {
            assert!(
                module.len() > 1024,
                "{series}-{variant}: module looks truncated"
            );
            assert_eq!(
                kernel_series(module_kernel_release(module).unwrap_or("")),
                Some(*series),
                "{series}-{variant}: module vermagic does not match its series"
            );
        }
    }

    #[test]
    fn selects_all_variants_of_a_kernel_series() {
        if MODULES.is_empty() {
            return;
        }
        for (series, variant, _) in MODULES {
            assert!(
                modules(series)
                    .iter()
                    .any(|(candidate, _)| candidate == variant),
                "{series}-{variant} is not selectable"
            );
        }
        assert!(modules("0.0").is_empty());
    }

    #[test]
    fn reports_the_running_release() {
        let release = running_release().expect("the kernel release is readable");
        assert!(!release.is_empty());
        assert!(!release.contains(' '));
    }
}
