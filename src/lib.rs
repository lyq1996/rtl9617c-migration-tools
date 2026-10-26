// SPDX-License-Identifier: GPL-2.0-only

pub mod board;
pub mod display;
pub mod dtb;
pub mod fip;
pub mod loader;
pub mod ubi;

#[cfg(target_os = "linux")]
pub mod install;
#[cfg(target_os = "linux")]
pub mod mtd;
#[cfg(target_os = "linux")]
pub mod unlock;

#[allow(dead_code)]
pub(crate) mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}
