// SPDX-License-Identifier: GPL-2.0-only

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Partition {
    pub index: u32,
    pub name: String,
    pub size: u64,
    pub erase_size: u64,
    pub offset: Option<u64>,
    pub flags: Option<u32>,
    pub path: PathBuf,
}

impl Partition {
    pub fn writeable(&self) -> bool {
        self.flags.is_some_and(|flags| flags & MTD_WRITEABLE != 0)
    }

    pub fn is_ubi_volume(&self) -> bool {
        self.name.starts_with("ubi_") && self.name != "ubi_device"
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct MtdInfoUser {
    type_: u8,
    _padding: [u8; 3],
    flags: u32,
    size: u32,
    erase_size: u32,
    write_size: u32,
    oob_size: u32,
    _padding2: u64,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct EraseInfoUser {
    start: u32,
    length: u32,
}

#[derive(Debug)]
pub struct MtdDevice {
    pub partition: Partition,
    device: File,
    pub write_size: u64,
    pub erase_size: u64,
    pub mtd_type: u8,
}

const IOC_NRBITS: u64 = 8;
const IOC_TYPEBITS: u64 = 8;
const IOC_SIZEBITS: u64 = 14;
const IOC_NRSHIFT: u64 = 0;
const IOC_TYPESHIFT: u64 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u64 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u64 = IOC_SIZESHIFT + IOC_SIZEBITS;
const IOC_WRITE: u64 = 1;
const IOC_READ: u64 = 2;

const MEMGETINFO: u64 = 1;
const MEMERASE: u64 = 2;
const MEMGETBADBLOCK: u64 = 11;

pub const MTD_WRITEABLE: u32 = 0x400;

const MTD_NANDFLASH: u8 = 4;
const MTD_MLCNANDFLASH: u8 = 8;

fn ioctl_number(direction: u64, command: u64, size: usize) -> libc::Ioctl {
    ((direction << IOC_DIRSHIFT)
        | ((b'M' as u64) << IOC_TYPESHIFT)
        | (command << IOC_NRSHIFT)
        | ((size as u64) << IOC_SIZESHIFT)) as libc::Ioctl
}

fn parse_number(text: &str) -> Option<u64> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else {
        text.parse().ok()
    }
}

fn read_partition_offset(index: u32) -> Option<u64> {
    let path = format!("/sys/class/mtd/mtd{index}/offset");
    fs::read_to_string(path)
        .ok()
        .and_then(|value| parse_number(&value))
        .or_else(|| (index == 0).then_some(0))
}

fn read_partition_flags(index: u32) -> Option<u32> {
    let path = format!("/sys/class/mtd/mtd{index}/flags");
    fs::read_to_string(path)
        .ok()
        .and_then(|value| parse_number(&value))
        .map(|flags| flags as u32)
}

pub fn writeable_summary() -> (usize, usize) {
    let Ok(partitions) = discover_partitions() else {
        return (0, 0);
    };
    let writeable = partitions
        .iter()
        .filter(|partition| partition.writeable())
        .count();
    (writeable, partitions.len())
}

pub fn discover_partitions() -> io::Result<Vec<Partition>> {
    let proc_mtd = fs::read_to_string("/proc/mtd")?;
    let mut partitions = Vec::new();

    for line in proc_mtd.lines().skip(1) {
        let Some((device, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(index) = device.strip_prefix("mtd").and_then(|v| v.parse().ok()) else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let Some(size) = fields.next().and_then(|v| u64::from_str_radix(v, 16).ok()) else {
            continue;
        };
        let Some(erase_size) = fields.next().and_then(|v| u64::from_str_radix(v, 16).ok()) else {
            continue;
        };
        let name = fields
            .collect::<Vec<_>>()
            .join(" ")
            .trim_matches('"')
            .to_string();
        partitions.push(Partition {
            index,
            name,
            size,
            erase_size,
            offset: read_partition_offset(index),
            flags: read_partition_flags(index),
            path: PathBuf::from(format!("/dev/mtd{index}")),
        });
    }

    partitions.sort_by_key(|partition| partition.index);
    Ok(partitions)
}

pub fn find_boot_partition(partitions: &[Partition]) -> Result<Partition, String> {
    let mut matches = partitions
        .iter()
        .filter(|partition| partition.name == "boot")
        .cloned();
    let Some(partition) = matches.next() else {
        let names = partitions
            .iter()
            .map(|partition| partition.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "No MTD partition is named \"boot\"; this device has: {names}"
        ));
    };
    if matches.next().is_some() {
        return Err("More than one MTD partition is named \"boot\"".to_string());
    }
    Ok(partition)
}

impl MtdDevice {
    pub fn open(partition: Partition) -> io::Result<Self> {
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&partition.path)?;
        if unsafe { libc::flock(device.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }

        let mut info = MtdInfoUser::default();
        let request = ioctl_number(IOC_READ, MEMGETINFO, std::mem::size_of::<MtdInfoUser>());
        if unsafe { libc::ioctl(device.as_raw_fd(), request, &mut info) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if info.erase_size == 0 || info.write_size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MTD reported invalid page or eraseblock geometry",
            ));
        }
        Ok(Self {
            partition,
            device,
            write_size: u64::from(info.write_size),
            erase_size: u64::from(info.erase_size),
            mtd_type: info.type_,
        })
    }

    pub fn is_nand(&self) -> bool {
        self.mtd_type == MTD_NANDFLASH || self.mtd_type == MTD_MLCNANDFLASH
    }

    pub fn read_exact_at(&self, offset: u64, data: &mut [u8]) -> io::Result<()> {
        let mut done = 0;
        while done < data.len() {
            let count = self
                .device
                .read_at(&mut data[done..], offset + done as u64)?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "MTD read ended early",
                ));
            }
            done += count;
        }
        Ok(())
    }

    fn write_all_at(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        let mut done = 0;
        while done < data.len() {
            let count = self.device.write_at(&data[done..], offset + done as u64)?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "MTD write returned zero bytes",
                ));
            }
            done += count;
        }
        Ok(())
    }

    pub fn is_bad(&self, offset: u64) -> io::Result<bool> {
        let mut offset = offset as libc::loff_t;
        let request = ioctl_number(
            IOC_WRITE,
            MEMGETBADBLOCK,
            std::mem::size_of::<libc::loff_t>(),
        );
        let result = unsafe { libc::ioctl(self.device.as_raw_fd(), request, &mut offset) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result == 1)
        }
    }

    pub fn check_range_good(&self, offset: u64, length: u64) -> Result<(), String> {
        let mut cursor = offset;
        while cursor < offset + length {
            match self.is_bad(cursor) {
                Ok(false) => {}
                Ok(true) => {
                    return Err(format!(
                        "Eraseblock at 0x{cursor:x} is marked bad; the boot offsets are fixed, \
                         so this unit cannot take the image at that offset"
                    ));
                }
                Err(error) => {
                    return Err(format!(
                        "Reading the bad-block marker at 0x{cursor:x} failed: {error}"
                    ));
                }
            }
            cursor += self.erase_size;
        }
        Ok(())
    }

    fn erase(&self, offset: u64) -> io::Result<()> {
        let start = u32::try_from(offset).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "Offset exceeds MEMERASE range")
        })?;
        let length = u32::try_from(self.erase_size).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Eraseblock exceeds MEMERASE range",
            )
        })?;
        let mut request = EraseInfoUser { start, length };
        let command = ioctl_number(IOC_WRITE, MEMERASE, std::mem::size_of::<EraseInfoUser>());
        if unsafe { libc::ioctl(self.device.as_raw_fd(), command, &mut request) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn erase_partition(&self) -> io::Result<()> {
        let mut offset = 0;
        while offset < self.partition.size {
            let bad = self.is_bad(offset).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("Bad-block check at offset 0x{offset:x} failed: {error}"),
                )
            })?;
            if !bad {
                self.erase(offset).map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!("Erase at offset 0x{offset:x} failed: {error}"),
                    )
                })?;
            }
            offset += self.erase_size;
        }
        Ok(())
    }

    pub fn write_eraseblock(&self, offset: u64, expected: &[u8]) -> io::Result<()> {
        if expected.len() as u64 != self.erase_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Write length does not match the eraseblock size",
            ));
        }
        self.erase(offset).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Erase at offset 0x{offset:x} failed: {error}"),
            )
        })?;
        self.write_all_at(offset, expected).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Write at offset 0x{offset:x} failed: {error}"),
            )
        })?;

        let mut actual = vec![0u8; expected.len()];
        self.read_exact_at(offset, &mut actual).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Readback at offset 0x{offset:x} failed: {error}"),
            )
        })?;
        if actual != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Readback at offset 0x{offset:x} differs from the written data"),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_writeable_from_sysfs_flags() {
        let mut partition = Partition {
            index: 0,
            name: "boot".to_string(),
            size: 0x20_0000,
            erase_size: 0x2_0000,
            offset: Some(0),
            flags: Some(MTD_WRITEABLE | 0x1),
            path: PathBuf::from("/dev/mtd0"),
        };
        assert!(partition.writeable());
        partition.flags = Some(0x1);
        assert!(!partition.writeable());
        partition.flags = None;
        assert!(!partition.writeable());
    }

    #[test]
    fn builds_the_ioctl_numbers_the_kernel_expects() {
        assert_eq!(ioctl_number(IOC_READ, MEMGETINFO, 32), 0x8020_4d01);
        assert_eq!(ioctl_number(IOC_WRITE, MEMERASE, 8), 0x4008_4d02);
        assert_eq!(ioctl_number(IOC_WRITE, MEMGETBADBLOCK, 8), 0x4008_4d0b);
    }

    #[test]
    fn finds_the_boot_partition_by_name() {
        let make = |index: u32, name: &str| Partition {
            index,
            name: name.to_string(),
            size: 0x20_0000,
            erase_size: 0x2_0000,
            offset: Some(0),
            flags: None,
            path: PathBuf::from(format!("/dev/mtd{index}")),
        };
        let partitions = vec![make(0, "boot"), make(1, "env")];
        assert_eq!(find_boot_partition(&partitions).unwrap().index, 0);
        assert!(find_boot_partition(&[make(1, "env")]).is_err());
        assert!(find_boot_partition(&[make(0, "boot"), make(4, "boot")]).is_err());
    }

    #[test]
    fn tells_ubi_volumes_from_physical_partitions() {
        let make = |name: &str| Partition {
            index: 0,
            name: name.to_string(),
            size: 0x20_0000,
            erase_size: 0x2_0000,
            offset: Some(0),
            flags: None,
            path: PathBuf::from("/dev/mtd0"),
        };
        assert!(!make("boot").is_ubi_volume());
        assert!(!make("ubi_device").is_ubi_volume());
        assert!(make("ubi_Config").is_ubi_volume());
        assert!(make("ubi_r0").is_ubi_volume());
    }
}
