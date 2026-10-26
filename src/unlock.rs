// SPDX-License-Identifier: GPL-2.0-only

use std::ffi::CString;
use std::io;

use crate::embedded::MODULES;

const VERMAGIC_TAG: &[u8] = b"vermagic=";

pub fn bundled() -> Vec<&'static str> {
    MODULES.iter().map(|(release, _)| *release).collect()
}

pub fn module(release: &str) -> Option<&'static [u8]> {
    MODULES
        .iter()
        .find(|(name, _)| *name == release)
        .map(|(_, bytes)| *bytes)
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
    let Some(module) = module(release) else {
        return Err(format!(
            "No MTD module is bundled for kernel {release}; this build carries {}",
            bundled().join(", ")
        ));
    };
    if let Some(recorded) = module_kernel_release(module)
        && recorded != release
    {
        return Err(format!(
            "The bundled module for {release} carries vermagic {recorded}; it would be rejected"
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
    let result = unsafe {
        libc::syscall(
            libc::SYS_init_module,
            module.as_ptr(),
            module.len(),
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
    fn every_bundled_module_is_named_after_its_own_vermagic() {
        assert!(!MODULES.is_empty(), "no modules were embedded");
        for (release, module) in MODULES {
            assert!(module.len() > 1024, "{release}: module looks truncated");
            assert_eq!(
                module_kernel_release(module),
                Some(*release),
                "modules/{release}.ko does not carry that release in its vermagic"
            );
        }
    }

    #[test]
    fn selects_the_module_for_the_requested_release() {
        for (release, _) in MODULES {
            assert!(module(release).is_some(), "{release} is not selectable");
        }
        assert!(module("0.0.0-not-a-release").is_none());
    }

    #[test]
    fn reports_the_running_release() {
        let release = running_release().expect("the kernel release is readable");
        assert!(!release.is_empty());
        assert!(!release.contains(' '));
    }
}
