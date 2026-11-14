// SPDX-License-Identifier: GPL-2.0-only

use std::sync::OnceLock;

use crate::embedded::BOARD_SOURCES;
use crate::fip::PayloadDigests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub name: String,
    pub compatible: String,
    pub bl2: [u8; 32],
    pub bl31: [u8; 32],
    pub bl33: [u8; 32],
}

fn parse_digest(source: &str, key: &str) -> Result<[u8; 32], String> {
    let hex = source.trim();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("`{key}` must be a 64-hex-digit SHA-256 digest"));
    }
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|error| format!("`{key}` is not a valid digest: {error}"))?;
    }
    Ok(digest)
}

fn parse(source: &str) -> Result<Board, String> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for (number, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected `key = value`", number + 1));
        };
        fields.push((key.trim().to_string(), value.trim().to_string()));
    }

    let take = |wanted: &str| -> Result<String, String> {
        fields
            .iter()
            .find(|(key, _)| key == wanted)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| format!("`{wanted}` is missing"))
    };

    for (key, _) in &fields {
        if !matches!(key.as_str(), "name" | "compatible" | "bl2" | "bl31" | "bl33") {
            return Err(format!("unknown key `{key}`"));
        }
    }

    Ok(Board {
        name: take("name")?,
        compatible: take("compatible")?,
        bl2: parse_digest(&take("bl2")?, "bl2")?,
        bl31: parse_digest(&take("bl31")?, "bl31")?,
        bl33: parse_digest(&take("bl33")?, "bl33")?,
    })
}

pub fn all() -> &'static [Board] {
    static BOARDS: OnceLock<Vec<Board>> = OnceLock::new();
    BOARDS.get_or_init(|| {
        BOARD_SOURCES
            .iter()
            .map(|(file, source)| {
                parse(source).unwrap_or_else(|error| {
                    panic!("boards/{file} is not a valid board definition: {error}")
                })
            })
            .collect()
    })
}

pub fn identify(digests: &PayloadDigests) -> Option<&'static Board> {
    all().iter().find(|board| {
        board.bl2 == digests.bl2 && board.bl31 == digests.bl31 && board.bl33 == digests.bl33
    })
}

pub fn known_boot(digests: &PayloadDigests) -> bool {
    all()
        .iter()
        .any(|board| board.bl2 == digests.bl2 && board.bl31 == digests.bl31)
}

fn boot_candidates(digests: &PayloadDigests) -> Vec<&'static Board> {
    all()
        .iter()
        .filter(|board| board.bl2 == digests.bl2 && board.bl31 == digests.bl31)
        .collect()
}

fn prefix(digest: &[u8; 32]) -> String {
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn describe(digests: &PayloadDigests) -> String {
    let candidates = boot_candidates(digests);
    if candidates.is_empty() {
        return format!(
            "unknown (BL2 {}…, BL31 {}…)",
            prefix(&digests.bl2),
            prefix(&digests.bl31)
        );
    }
    // Prefer the boards whose full fingerprint (BL2+BL31+BL33) matches. Since
    // this tool only rewrites BL33, a replaced BL33 falls back to the boards
    // sharing BL2+BL31. Either level may match more than one board.
    let exact: Vec<&'static Board> = candidates
        .iter()
        .copied()
        .filter(|board| board.bl33 == digests.bl33)
        .collect();
    let shown = if exact.is_empty() {
        &candidates
    } else {
        &exact
    };
    match shown.as_slice() {
        [board] => format!("{} ({})", board.name, board.compatible),
        _ => shown
            .iter()
            .map(|board| board.name.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(text: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn every_configured_board_parses() {
        let boards = all();
        assert!(boards.len() >= 3, "no board definitions were embedded");
        for board in boards {
            assert!(!board.name.is_empty());
            assert!(board.compatible.contains(','));
            assert_ne!(board.bl2, [0u8; 32]);
            assert_ne!(board.bl31, [0u8; 32]);
            assert_ne!(board.bl33, [0u8; 32]);
        }
    }

    #[test]
    fn names_and_compatible_are_unique() {
        let boards = all();
        for (index, board) in boards.iter().enumerate() {
            for other in &boards[index + 1..] {
                assert_ne!(board.name, other.name);
                assert_ne!(board.compatible, other.compatible);
            }
        }
    }

    #[test]
    fn identifies_boards_by_their_fingerprints() {
        for board in all() {
            let digests = PayloadDigests {
                bl2: board.bl2,
                bl31: board.bl31,
                bl33: board.bl33,
            };
            let found = identify(&digests).expect("a board with these fingerprints");
            assert_eq!(
                (found.bl2, found.bl31, found.bl33),
                (board.bl2, board.bl31, board.bl33)
            );
        }
    }

    #[test]
    fn exact_identification_requires_bl33() {
        let board = &all()[0];
        let mut digests = PayloadDigests {
            bl2: board.bl2,
            bl31: board.bl31,
            bl33: board.bl33,
        };
        digests.bl33[0] ^= 0xff;
        assert!(identify(&digests).is_none());
        assert!(known_boot(&digests));
        let text = describe(&digests);
        for candidate in boot_candidates(&digests) {
            assert!(text.contains(&candidate.name), "{text}");
        }
    }

    #[test]
    fn shared_boards_are_listed_when_bl33_is_unknown() {
        let find = |name: &str| all().iter().find(|board| board.name == name).unwrap();
        let pt1500 = find("PT1500-H10-CT-2P5G");
        let digests = PayloadDigests {
            bl2: pt1500.bl2,
            bl31: pt1500.bl31,
            bl33: [0x55; 32],
        };
        let text = describe(&digests);
        assert!(text.contains("PT1500-H10-CT-2P5G"), "{text}");
        assert!(text.contains("PT1570-H10-CT-2P5G"), "{text}");
    }

    #[test]
    fn identical_fingerprints_are_listed_as_candidates() {
        let find = |name: &str| all().iter().find(|board| board.name == name).unwrap();
        let tyfza1_2p5g = find("TYFZA1-2P5G");
        let tyfza1_4ge = find("TYFZA1-4GE");
        assert_eq!(
            (tyfza1_2p5g.bl2, tyfza1_2p5g.bl31, tyfza1_2p5g.bl33),
            (tyfza1_4ge.bl2, tyfza1_4ge.bl31, tyfza1_4ge.bl33)
        );
        let digests = PayloadDigests {
            bl2: tyfza1_2p5g.bl2,
            bl31: tyfza1_2p5g.bl31,
            bl33: tyfza1_2p5g.bl33,
        };
        let text = describe(&digests);
        assert!(text.contains("TYFZA1-2P5G"), "{text}");
        assert!(text.contains("TYFZA1-4GE"), "{text}");
        assert!(text.contains('\n'), "{text}");
    }

    #[test]
    fn describe_names_known_boards_and_labels_the_rest() {
        let board = all()
            .iter()
            .find(|board| board.name == "PT1500-H10-CT-2P5G")
            .unwrap();
        let digests = PayloadDigests {
            bl2: board.bl2,
            bl31: board.bl31,
            bl33: board.bl33,
        };
        assert_eq!(
            describe(&digests),
            format!("{} ({})", board.name, board.compatible)
        );
        let mut other = digests;
        other.bl2[0] = 0x12;
        other.bl2[1] = 0x34;
        let text = describe(&other);
        assert!(text.starts_with("unknown"), "{text}");
        assert!(text.contains("1234"), "{text}");
    }

    #[test]
    fn shared_bl2_and_bl31_are_split_by_bl33() {
        let find = |name: &str| all().iter().find(|board| board.name == name).unwrap();
        let pt1500 = find("PT1500-H10-CT-2P5G");
        let pt1570 = find("PT1570-H10-CT-2P5G");
        assert_eq!(pt1500.bl2, pt1570.bl2);
        assert_eq!(pt1500.bl31, pt1570.bl31);
        assert_ne!(pt1500.bl33, pt1570.bl33);
    }

    #[test]
    fn parses_a_board_definition() {
        let board = parse(
            "name = Test\n\
             compatible = vendor,test\n\
             bl2 = b04b7abaad152f5708d8aff216a0765b5bef4f726beeda1c5f199376c8434934\n\
             bl31 = 1F6444E2244FD19AF655132AAAA1758A81798DD5C851FBE46E85567F915C7DB9\n\
             bl33 = 183713CCEA4B270136F43C4E4E2863080B24497356E384F94409B3E236EB3571\n",
        )
        .unwrap();
        assert_eq!(board.name, "Test");
        assert_eq!(
            board.bl2,
            digest("b04b7abaad152f5708d8aff216a0765b5bef4f726beeda1c5f199376c8434934")
        );
        assert_eq!(
            board.bl31,
            digest("1f6444e2244fd19af655132aaaa1758a81798dd5c851fbe46e85567f915c7db9")
        );
        assert_eq!(
            board.bl33,
            digest("183713ccea4b270136f43c4e4e2863080b24497356e384f94409b3e236eb3571")
        );
    }

    #[test]
    fn rejects_unknown_keys_and_bad_digests() {
        assert!(parse("name = A\nbogus = B\ncompatible = a,b\nbl2 = 00\nbl31 = 11\nbl33 = 22\n").is_err());
        assert!(parse("name = A\ncompatible = a,b\nbl2 = 00\nbl31 = 11\nbl33 = 22\n").is_err());
        assert!(parse("name = A\ncompatible = a,b\nbl2 = nothex\nbl31 = 11\nbl33 = 22\n").is_err());
        assert!(parse("name = A\ncompatible = a,b\nbl2 = b04b\nbl31 = 11\nbl33 = 22\n").is_err());
    }
}
