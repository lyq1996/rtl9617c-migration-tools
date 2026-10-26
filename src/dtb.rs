// SPDX-License-Identifier: GPL-2.0-only

const FDT_MAGIC: [u8; 4] = [0xd0, 0x0d, 0xfe, 0xed];
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;
const HEADER_BYTES: usize = 40;

fn be_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

pub fn find_appended(image: &[u8]) -> Option<&[u8]> {
    let mut found = None;
    for offset in 0..image.len().saturating_sub(4) {
        if image[offset..offset + 4] != FDT_MAGIC {
            continue;
        }
        let total = be_u32(image, offset + 4)? as usize;
        let end = offset.checked_add(total)?;
        if total >= HEADER_BYTES && end <= image.len() && image.len() - end <= 8 {
            found = Some(&image[offset..end]);
        }
    }
    found
}

pub fn property<'a>(dtb: &'a [u8], wanted: &str) -> Option<&'a [u8]> {
    if dtb.get(..4)? != FDT_MAGIC {
        return None;
    }
    let total = be_u32(dtb, 4)? as usize;
    let struct_offset = be_u32(dtb, 8)? as usize;
    let strings_offset = be_u32(dtb, 12)? as usize;
    if total > dtb.len() {
        return None;
    }

    let mut cursor = struct_offset;
    let mut depth = 0usize;
    loop {
        let token = be_u32(dtb, cursor)?;
        cursor += 4;
        match token {
            FDT_BEGIN_NODE => {
                depth += 1;
                let start = cursor;
                while *dtb.get(cursor)? != 0 {
                    cursor += 1;
                }
                cursor = start + (cursor - start + 1).next_multiple_of(4);
            }
            FDT_END_NODE => depth = depth.checked_sub(1)?,
            FDT_NOP => {}
            FDT_END => return None,
            FDT_PROP => {
                let length = be_u32(dtb, cursor)? as usize;
                let name_offset = be_u32(dtb, cursor + 4)? as usize;
                let value = cursor + 8;
                if value + length > dtb.len() {
                    return None;
                }
                if depth == 1 && property_name(dtb, strings_offset, name_offset)? == wanted {
                    return Some(&dtb[value..value + length]);
                }
                cursor = value + length.next_multiple_of(4);
            }
            _ => return None,
        }
    }
}

fn property_name(dtb: &[u8], strings_offset: usize, name_offset: usize) -> Option<&str> {
    let start = strings_offset.checked_add(name_offset)?;
    let rest = dtb.get(start..)?;
    let end = rest.iter().position(|byte| *byte == 0)?;
    std::str::from_utf8(&rest[..end]).ok()
}

fn first_string(value: &[u8]) -> Option<&str> {
    let end = value.iter().position(|byte| *byte == 0)?;
    std::str::from_utf8(&value[..end]).ok()
}

pub fn model(dtb: &[u8]) -> Option<&str> {
    first_string(property(dtb, "model")?)
}

pub fn compatible(dtb: &[u8]) -> Option<Vec<&str>> {
    let value = property(dtb, "compatible")?;
    let mut entries = Vec::new();
    for part in value.split(|byte| *byte == 0) {
        if part.is_empty() {
            continue;
        }
        entries.push(std::str::from_utf8(part).ok()?);
    }
    if entries.is_empty() {
        return None;
    }
    Some(entries)
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;

    pub fn blob(model: &str, compatible: &[&str]) -> Vec<u8> {
        let mut strings = Vec::new();
        let model_name_offset = 0u32;
        strings.extend_from_slice(b"model\0");
        let compatible_name_offset = strings.len() as u32;
        strings.extend_from_slice(b"compatible\0");

        let mut structure = Vec::new();
        structure.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
        structure.extend_from_slice(&[0, 0, 0, 0]);

        let push = |structure: &mut Vec<u8>, name_offset: u32, value: &[u8]| {
            structure.extend_from_slice(&FDT_PROP.to_be_bytes());
            structure.extend_from_slice(&(value.len() as u32).to_be_bytes());
            structure.extend_from_slice(&name_offset.to_be_bytes());
            structure.extend_from_slice(value);
            while !structure.len().is_multiple_of(4) {
                structure.push(0);
            }
        };

        let mut model_value = model.as_bytes().to_vec();
        model_value.push(0);
        push(&mut structure, model_name_offset, &model_value);

        let mut compatible_value = Vec::new();
        for entry in compatible {
            compatible_value.extend_from_slice(entry.as_bytes());
            compatible_value.push(0);
        }
        push(&mut structure, compatible_name_offset, &compatible_value);

        structure.extend_from_slice(&FDT_END_NODE.to_be_bytes());
        structure.extend_from_slice(&FDT_END.to_be_bytes());

        let mem_rsvmap = [0u8; 16];
        let struct_offset = HEADER_BYTES + mem_rsvmap.len();
        let strings_offset = struct_offset + structure.len();
        let total = strings_offset + strings.len();

        let mut dtb = Vec::with_capacity(total);
        dtb.extend_from_slice(&FDT_MAGIC);
        dtb.extend_from_slice(&(total as u32).to_be_bytes());
        dtb.extend_from_slice(&(struct_offset as u32).to_be_bytes());
        dtb.extend_from_slice(&(strings_offset as u32).to_be_bytes());
        dtb.extend_from_slice(&(HEADER_BYTES as u32).to_be_bytes());
        dtb.extend_from_slice(&17u32.to_be_bytes());
        dtb.extend_from_slice(&16u32.to_be_bytes());
        dtb.extend_from_slice(&0u32.to_be_bytes());
        dtb.extend_from_slice(&(strings.len() as u32).to_be_bytes());
        dtb.extend_from_slice(&(structure.len() as u32).to_be_bytes());
        dtb.extend_from_slice(&mem_rsvmap);
        dtb.extend_from_slice(&structure);
        dtb.extend_from_slice(&strings);
        dtb
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::blob;
    use super::*;

    const PT: [&str; 2] = ["youhua,pt1500", "realtek,rtl9617c"];

    #[test]
    fn reads_the_model_and_compatible_of_a_bare_tree() {
        let dtb = blob("Youhua PT1500-H10", &PT);
        assert_eq!(model(&dtb), Some("Youhua PT1500-H10"));
        assert_eq!(compatible(&dtb), Some(PT.to_vec()));
    }

    #[test]
    fn finds_a_tree_appended_to_an_image() {
        let dtb = blob("Tianyi TYFZA1", &["tianyi,tyfza1", "realtek,rtl9617c"]);
        let mut image = vec![0x0au8; 0x140];
        image[..4].copy_from_slice(&0x1400_000au32.to_le_bytes());
        let start = (image.len() - dtb.len()) & !7;
        image[start..start + dtb.len()].copy_from_slice(&dtb);
        let found = find_appended(&image).unwrap();
        assert_eq!(model(found), Some("Tianyi TYFZA1"));
    }

    #[test]
    fn reports_no_tree_for_an_image_without_one() {
        assert!(find_appended(&vec![0x0au8; 0x140]).is_none());
    }

    #[test]
    fn ignores_a_truncated_tree() {
        let dtb = blob("Tianyi TYFZA1", &["tianyi,tyfza1"]);
        assert!(find_appended(&dtb[..dtb.len() - 4]).is_none());
    }

    #[test]
    fn survives_a_damaged_structure_block() {
        let mut dtb = blob("Tianyi TYFZA1", &["tianyi,tyfza1"]);
        dtb[8..12].copy_from_slice(&0xffff_ffffu32.to_be_bytes());
        assert!(model(&dtb).is_none());
        assert!(compatible(&dtb).is_none());
    }

    #[test]
    fn reports_an_absent_compatible() {
        let dtb = blob("No Compatible", &[]);
        assert_eq!(model(&dtb), Some("No Compatible"));
        assert!(compatible(&dtb).is_none());
    }
}
