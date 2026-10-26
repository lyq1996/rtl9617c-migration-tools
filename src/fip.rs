// SPDX-License-Identifier: GPL-2.0-only

pub const BOOT_SIZE: usize = 0x20_0000;
pub const FIP_SIZE: usize = 0xc_0000;
pub const FIP_OFFSETS: [usize; 2] = [0x4_0000, 0x10_0000];
pub const ERASE_SIZE: usize = 0x2_0000;

const GPT_HEADERS: [usize; 2] = [0x200, BOOT_SIZE - 0x200];
const GPT_TABLES: [usize; 2] = [0x400, BOOT_SIZE - 0x4200];
const GPT_HEADER_BYTES: usize = 92;
const GPT_TABLE_BYTES: usize = 0x4000;
const GPT_ENTRY_BYTES: usize = 128;
const GPT_ENTRIES: u32 = 128;

const TOC_MAGIC: [u8; 4] = [0x01, 0x00, 0x64, 0xaa];
const TOC_HEADER_SIZE: usize = 16;
const TOC_ENTRY_SIZE: usize = 40;
const TOC_MAX_ENTRIES: usize = 32;
const TOC_LIMIT: usize = TOC_HEADER_SIZE + TOC_MAX_ENTRIES * TOC_ENTRY_SIZE;

pub const UUID_TB_FW: [u8; 16] = [
    0x5f, 0xf9, 0xec, 0x0b, 0x4d, 0x22, 0x3e, 0x4d, 0xa5, 0x44, 0xc3, 0x9d, 0x81, 0xc7, 0x3f, 0x0a,
];
pub const UUID_BL31: [u8; 16] = [
    0x47, 0xd4, 0x08, 0x6d, 0x4c, 0xfe, 0x98, 0x46, 0x9b, 0x95, 0x29, 0x50, 0xcb, 0xbd, 0x5a, 0x00,
];
pub const UUID_BL33: [u8; 16] = [
    0xd6, 0xd0, 0xee, 0xa7, 0xfc, 0xea, 0xd5, 0x4b, 0x97, 0x82, 0x99, 0x34, 0xf2, 0x34, 0xb6, 0xe4,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FipEntry {
    pub uuid: [u8; 16],
    pub offset: usize,
    pub size: usize,
    pub entry_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fip {
    pub entries: Vec<FipEntry>,
}

fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

impl Fip {
    pub fn payload(&self, uuid: &[u8; 16]) -> Option<&FipEntry> {
        self.entries.iter().find(|entry| &entry.uuid == uuid)
    }

    pub fn require(&self, uuid: &[u8; 16], name: &str) -> Result<&FipEntry, String> {
        self.payload(uuid)
            .ok_or_else(|| format!("FIP is missing the {name} payload"))
    }

    pub fn capacity(&self, entry: &FipEntry, slot_size: usize) -> usize {
        self.entries
            .iter()
            .filter(|other| other.offset > entry.offset)
            .map(|other| other.offset)
            .min()
            .unwrap_or(slot_size)
            .saturating_sub(entry.offset)
    }
}

pub fn parse(slot: &[u8]) -> Result<Fip, String> {
    let size = slot.len();
    if size < TOC_HEADER_SIZE + TOC_ENTRY_SIZE {
        return Err("FIP slot is smaller than one directory entry".to_string());
    }
    if slot[..TOC_MAGIC.len()] != TOC_MAGIC {
        return Err("FIP ToC magic 0xaa640001 is missing".to_string());
    }

    let mut terminator = None;
    let mut cursor = TOC_HEADER_SIZE;
    while cursor + TOC_ENTRY_SIZE <= size && cursor < TOC_LIMIT {
        if slot[cursor..cursor + 16].iter().all(|byte| *byte == 0) {
            terminator = Some(cursor);
            break;
        }
        cursor += TOC_ENTRY_SIZE;
    }
    let terminator = terminator.ok_or("FIP ToC terminator is missing")?;
    let table_end = terminator + TOC_ENTRY_SIZE;

    let mut entries: Vec<FipEntry> = Vec::new();
    let mut cursor = TOC_HEADER_SIZE;
    while cursor < terminator {
        let raw = &slot[cursor..cursor + TOC_ENTRY_SIZE];
        let uuid: [u8; 16] = raw[..16].try_into().unwrap();
        let offset = usize::try_from(read_u64_le(raw, 16))
            .map_err(|_| "FIP payload offset exceeds the platform range")?;
        let length = usize::try_from(read_u64_le(raw, 24))
            .map_err(|_| "FIP payload length exceeds the platform range")?;
        if length == 0 || offset < table_end || offset > size || length > size - offset {
            return Err(format!(
                "FIP payload at 0x{offset:x} ({length} bytes) does not fit the slot"
            ));
        }
        let clash = entries.iter().any(|other| {
            other.uuid == uuid
                || (offset < other.offset + other.size && other.offset < offset + length)
        });
        if clash {
            return Err("FIP payloads overlap or repeat a UUID".to_string());
        }
        entries.push(FipEntry {
            uuid,
            offset,
            size: length,
            entry_offset: cursor,
        });
        cursor += TOC_ENTRY_SIZE;
    }

    let fip = Fip { entries };
    fip.require(&UUID_TB_FW, "TB_FW/BL2")?;
    fip.require(&UUID_BL31, "BL31")?;
    fip.require(&UUID_BL33, "BL33/U-Boot")?;
    Ok(fip)
}

pub fn parse_boot_image(image: &[u8]) -> Result<[Fip; 2], String> {
    if image.len() != BOOT_SIZE {
        return Err(format!(
            "Boot image is {} bytes; the boot partition is {BOOT_SIZE}",
            image.len()
        ));
    }
    for header in GPT_HEADERS {
        if &image[header..header + 8] != b"EFI PART" {
            return Err(format!("GPT signature is missing at 0x{header:x}"));
        }
    }

    let first = parse(&image[FIP_OFFSETS[0]..FIP_OFFSETS[0] + FIP_SIZE])?;
    let second = parse(&image[FIP_OFFSETS[1]..FIP_OFFSETS[1] + FIP_SIZE])?;
    let slot = &image[FIP_OFFSETS[0]..];
    let mirror = &image[FIP_OFFSETS[1]..];
    for (uuid, name) in [
        (UUID_TB_FW, "BL2"),
        (UUID_BL31, "BL31"),
        (UUID_BL33, "BL33"),
    ] {
        let a = first.payload(&uuid).unwrap();
        let b = second.payload(&uuid).unwrap();
        if a.size != b.size
            || slot[a.offset..a.offset + a.size] != mirror[b.offset..b.offset + b.size]
        {
            return Err(format!("The two FIP copies disagree on {name}"));
        }
    }
    Ok([first, second])
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn gpt_checksums_valid(image: &[u8]) -> bool {
    let mut header = [0u8; GPT_HEADER_BYTES];
    for (index, header_offset) in GPT_HEADERS.into_iter().enumerate() {
        let table_offset = GPT_TABLES[index];
        if &image[header_offset..header_offset + 8] != b"EFI PART"
            || read_u32_le(image, header_offset + 12) != GPT_HEADER_BYTES as u32
            || read_u64_le(image, header_offset + 72) != (table_offset / 512) as u64
            || read_u32_le(image, header_offset + 80) != GPT_ENTRIES
            || read_u32_le(image, header_offset + 84) != GPT_ENTRY_BYTES as u32
        {
            return false;
        }
        header.copy_from_slice(&image[header_offset..header_offset + GPT_HEADER_BYTES]);
        header[16..20].fill(0);
        if crc32(&header) != read_u32_le(image, header_offset + 16)
            || crc32(&image[table_offset..table_offset + GPT_TABLE_BYTES])
                != read_u32_le(image, header_offset + 88)
        {
            return false;
        }
    }
    true
}

fn gpt_fip_sizes_valid(image: &[u8], next: &[u8]) -> bool {
    for (index, offset) in FIP_OFFSETS.iter().enumerate() {
        let fip = &next[*offset..*offset + FIP_SIZE];
        let mut used = 0usize;
        let mut cursor = TOC_HEADER_SIZE;
        while cursor < TOC_LIMIT && !fip[cursor..cursor + 16].iter().all(|byte| *byte == 0) {
            let end = read_u64_le(fip, cursor + 16) + read_u64_le(fip, cursor + 24);
            used = used.max(usize::try_from(end).unwrap_or(usize::MAX));
            cursor += TOC_ENTRY_SIZE;
        }

        for table in GPT_TABLES {
            let entry = table + index * GPT_ENTRY_BYTES;
            let first = read_u64_le(image, entry + 32);
            let last = read_u64_le(image, entry + 40);
            let slot_end = ((offset + FIP_SIZE) / 512) as u64;
            if first != (offset / 512) as u64
                || last < first
                || last >= slot_end
                || last - first + 1 < (used as u64).div_ceil(512)
                || last != read_u64_le(image, GPT_TABLES[0] + index * GPT_ENTRY_BYTES + 40)
            {
                return false;
            }
        }
    }
    true
}

fn gpt_variable(offset: usize) -> bool {
    for (index, header) in GPT_HEADERS.into_iter().enumerate() {
        if (offset >= header + 0x10 && offset < header + 0x14)
            || (offset >= header + 0x38 && offset < header + 0x48)
            || (offset >= header + 0x58 && offset < header + 0x5c)
        {
            return true;
        }
        let table = GPT_TABLES[index];
        if offset >= table && offset < table + GPT_TABLE_BYTES {
            let field = (offset - table) % GPT_ENTRY_BYTES;
            if (offset < table + 2 * GPT_ENTRY_BYTES && (0x28..0x30).contains(&field))
                || (0x10..0x20).contains(&field)
            {
                return true;
            }
        }
    }
    false
}

fn same_boot_layout(current: &[u8], next: &[u8], start: usize, end: usize) -> bool {
    (start..end).all(|offset| current[offset] == next[offset] || gpt_variable(offset))
}

pub fn preserves_device(next: &[u8], current: &[u8]) -> Result<(), String> {
    if current.len() != BOOT_SIZE {
        return Err("The current boot partition is not 2 MiB".to_string());
    }
    let image = parse_boot_image(next)?;
    if !gpt_checksums_valid(current) {
        return Err("The GPT on this device fails its own checksums".to_string());
    }
    if !gpt_checksums_valid(next) {
        return Err("The new image's GPT fails its own checksums".to_string());
    }
    if !gpt_fip_sizes_valid(current, next) {
        return Err("The new FIP does not fit the FIP partitions on this device".to_string());
    }
    if !gpt_fip_sizes_valid(next, next) {
        return Err("The new image's GPT does not describe its own FIP copies".to_string());
    }
    if !same_boot_layout(current, next, 0, FIP_OFFSETS[0]) {
        return Err("The new image changes the GPT area of this device".to_string());
    }
    if !same_boot_layout(current, next, 0x1c_0000, BOOT_SIZE) {
        return Err("The new image changes the reserved tail of this device".to_string());
    }

    let mut matched = 0;
    for offset in FIP_OFFSETS {
        let Ok(old) = parse(&current[offset..offset + FIP_SIZE]) else {
            continue;
        };
        let same = [UUID_TB_FW, UUID_BL31].iter().all(|uuid| {
            let a = old.payload(uuid).unwrap();
            let b = image[0].payload(uuid).unwrap();
            a.size == b.size
                && current[offset + a.offset..offset + a.offset + a.size]
                    == next[FIP_OFFSETS[0] + b.offset..FIP_OFFSETS[0] + b.offset + b.size]
        });
        if same {
            matched += 1;
        }
    }
    if matched == 0 {
        return Err(
            "The new image's BL2 and BL31 are not the ones on this device; \
                    it was built for another board"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;

    pub fn boot_image(bl2: &[u8], bl31: &[u8], bl33: &[u8], certs: usize) -> Vec<u8> {
        let mut fip = vec![0u8; FIP_SIZE];
        fip[..4].copy_from_slice(&TOC_MAGIC);
        let mut cursor = TOC_HEADER_SIZE;
        let mut body = TOC_HEADER_SIZE + (3 + certs + 1) * TOC_ENTRY_SIZE;
        let mut push = |fip: &mut Vec<u8>, cursor: &mut usize, uuid: [u8; 16], data: &[u8]| {
            fip[*cursor..*cursor + 16].copy_from_slice(&uuid);
            fip[*cursor + 16..*cursor + 24].copy_from_slice(&(body as u64).to_le_bytes());
            fip[*cursor + 24..*cursor + 32].copy_from_slice(&(data.len() as u64).to_le_bytes());
            fip[body..body + data.len()].copy_from_slice(data);
            body += data.len();
            *cursor += TOC_ENTRY_SIZE;
        };
        push(&mut fip, &mut cursor, UUID_TB_FW, bl2);
        push(&mut fip, &mut cursor, UUID_BL31, bl31);
        push(&mut fip, &mut cursor, UUID_BL33, bl33);
        for index in 0..certs {
            let mut uuid = [0u8; 16];
            uuid[0] = 0xb0 + index as u8;
            push(&mut fip, &mut cursor, uuid, &[0x55; 24]);
        }
        fip[cursor + 16..cursor + 24].copy_from_slice(&(body as u64).to_le_bytes());

        let mut image = vec![0u8; BOOT_SIZE];
        for offset in FIP_OFFSETS {
            image[offset..offset + FIP_SIZE].copy_from_slice(&fip);
        }
        for table in GPT_TABLES {
            for (index, offset) in FIP_OFFSETS.iter().enumerate() {
                let entry = table + index * GPT_ENTRY_BYTES;
                image[entry..entry + 16].fill(0x40 + index as u8);
                image[entry + 32..entry + 40]
                    .copy_from_slice(&((offset / 512) as u64).to_le_bytes());
                image[entry + 40..entry + 48]
                    .copy_from_slice(&(((offset + FIP_SIZE) / 512 - 1) as u64).to_le_bytes());
            }
        }
        for (index, header) in GPT_HEADERS.into_iter().enumerate() {
            let table = GPT_TABLES[index];
            image[header..header + 8].copy_from_slice(b"EFI PART");
            image[header + 12..header + 16]
                .copy_from_slice(&(GPT_HEADER_BYTES as u32).to_le_bytes());
            image[header + 72..header + 80].copy_from_slice(&((table / 512) as u64).to_le_bytes());
            image[header + 80..header + 84].copy_from_slice(&GPT_ENTRIES.to_le_bytes());
            image[header + 84..header + 88]
                .copy_from_slice(&(GPT_ENTRY_BYTES as u32).to_le_bytes());
            let entries = crc32(&image[table..table + GPT_TABLE_BYTES]);
            image[header + 88..header + 92].copy_from_slice(&entries.to_le_bytes());
            let mut copy = image[header..header + GPT_HEADER_BYTES].to_vec();
            copy[16..20].fill(0);
            let header_crc = crc32(&copy);
            image[header + 16..header + 20].copy_from_slice(&header_crc.to_le_bytes());
        }
        image
    }

    pub fn arm64(len: usize, fill: u8) -> Vec<u8> {
        let mut data = vec![fill; len];
        data[..4].copy_from_slice(&0x1400_000au32.to_le_bytes());
        data
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{arm64, boot_image};
    use super::*;

    #[test]
    fn parses_the_three_required_payloads() {
        let image = boot_image(
            &arm64(0x200, 0x11),
            &arm64(0x180, 0x22),
            &arm64(0x1000, 0x33),
            6,
        );
        let [first, second] = parse_boot_image(&image).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.entries.len(), 9);
        assert_eq!(first.require(&UUID_BL33, "BL33").unwrap().size, 0x1000);
    }

    #[test]
    fn rejects_a_directory_without_bl33() {
        let image = boot_image(
            &arm64(0x200, 0x11),
            &arm64(0x180, 0x22),
            &arm64(0x1000, 0x33),
            0,
        );
        let mut slot = image[FIP_OFFSETS[0]..FIP_OFFSETS[0] + FIP_SIZE].to_vec();
        let entry = 16 + 2 * TOC_ENTRY_SIZE;
        slot[entry..entry + 16].fill(0);
        assert!(parse(&slot).is_err());
    }

    #[test]
    fn reports_the_room_bl33_has_before_the_certificates() {
        let image = boot_image(
            &arm64(0x200, 0x11),
            &arm64(0x180, 0x22),
            &arm64(0x1000, 0x33),
            6,
        );
        let [first, _] = parse_boot_image(&image).unwrap();
        let entry = first.require(&UUID_BL33, "BL33").unwrap();
        assert_eq!(first.capacity(entry, FIP_SIZE), 0x1000);
    }

    #[test]
    fn accepts_an_image_that_only_replaces_bl33() {
        let current = boot_image(
            &arm64(0x200, 0x11),
            &arm64(0x180, 0x22),
            &arm64(0x1000, 0x33),
            6,
        );
        let mut next = current.clone();
        let mut slot = next[FIP_OFFSETS[0]..FIP_OFFSETS[0] + FIP_SIZE].to_vec();
        let entry = 16 + 2 * TOC_ENTRY_SIZE;
        let offset = read_u64_le(&slot, entry + 16) as usize;
        let replacement = arm64(0x800, 0x44);
        slot[entry + 24..entry + 32].copy_from_slice(&(replacement.len() as u64).to_le_bytes());
        slot[offset..offset + replacement.len()].copy_from_slice(&replacement);
        for start in FIP_OFFSETS {
            next[start..start + FIP_SIZE].copy_from_slice(&slot);
        }
        preserves_device(&next, &current).unwrap();
    }

    #[test]
    fn refuses_a_different_bl2() {
        let current = boot_image(
            &arm64(0x200, 0x11),
            &arm64(0x180, 0x22),
            &arm64(0x1000, 0x33),
            6,
        );
        let next = boot_image(
            &arm64(0x200, 0x99),
            &arm64(0x180, 0x22),
            &arm64(0x1000, 0x33),
            6,
        );
        assert!(preserves_device(&next, &current).is_err());
    }

    #[test]
    fn refuses_a_changed_gpt_area() {
        let current = boot_image(
            &arm64(0x200, 0x11),
            &arm64(0x180, 0x22),
            &arm64(0x1000, 0x33),
            6,
        );
        let mut next = current.clone();
        next[0x3000] ^= 0xff;
        assert!(preserves_device(&next, &current).is_err());
    }

    #[test]
    fn refuses_a_changed_tail() {
        let current = boot_image(
            &arm64(0x200, 0x11),
            &arm64(0x180, 0x22),
            &arm64(0x1000, 0x33),
            6,
        );
        let mut next = current.clone();
        next[0x1e_0000] ^= 0xff;
        assert!(preserves_device(&next, &current).is_err());
    }

    #[test]
    fn rejects_a_payload_that_overlaps_the_directory() {
        let image = boot_image(
            &arm64(0x200, 0x11),
            &arm64(0x180, 0x22),
            &arm64(0x1000, 0x33),
            0,
        );
        let mut slot = image[FIP_OFFSETS[0]..FIP_OFFSETS[0] + FIP_SIZE].to_vec();
        slot[TOC_HEADER_SIZE + 16..TOC_HEADER_SIZE + 24].copy_from_slice(&0x10u64.to_le_bytes());
        assert!(parse(&slot).is_err());
    }
}
