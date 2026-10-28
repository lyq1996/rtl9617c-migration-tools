// SPDX-License-Identifier: GPL-2.0-only

use std::sync::OnceLock;

use crate::embedded::BOARD_SOURCES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub name: String,
    pub compatible: String,
    pub bl2: [u8; 32],
    pub bl31: [u8; 32],
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
        if !matches!(key.as_str(), "name" | "compatible" | "bl2" | "bl31") {
            return Err(format!("unknown key `{key}`"));
        }
    }

    Ok(Board {
        name: take("name")?,
        compatible: take("compatible")?,
        bl2: parse_digest(&take("bl2")?, "bl2")?,
        bl31: parse_digest(&take("bl31")?, "bl31")?,
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

pub fn identify(bl2: &[u8; 32]) -> Option<&'static Board> {
    all().iter().find(|board| &board.bl2 == bl2)
}

pub fn describe(bl2: &[u8; 32]) -> String {
    let prefix = bl2[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    match identify(bl2) {
        Some(board) => board.name.clone(),
        None => format!("unknown variant (BL2 {prefix}…)"),
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
        assert!(boards.len() >= 2, "no board definitions were embedded");
        for board in boards {
            assert!(!board.name.is_empty());
            assert!(board.compatible.contains(','));
            assert_ne!(board.bl2, [0u8; 32]);
            assert_ne!(board.bl31, [0u8; 32]);
        }
    }

    #[test]
    fn names_compatible_and_fingerprints_are_unique() {
        let boards = all();
        for (index, board) in boards.iter().enumerate() {
            for other in &boards[index + 1..] {
                assert_ne!(board.name, other.name);
                assert_ne!(board.compatible, other.compatible);
                assert_ne!(board.bl2, other.bl2);
                assert_ne!(board.bl31, other.bl31);
            }
        }
    }

    #[test]
    fn identifies_boards_by_their_bl2_digest() {
        for board in all() {
            assert_eq!(identify(&board.bl2).unwrap().name, board.name);
        }
    }

    #[test]
    fn unknown_bl2_yields_no_board() {
        let mut other = all()[0].bl2;
        other[0] ^= 0xff;
        assert!(identify(&other).is_none());
    }

    #[test]
    fn describe_names_known_boards_and_labels_the_rest() {
        assert_eq!(describe(&all()[0].bl2), all()[0].name);
        let mut other = all()[0].bl2;
        other[0] = 0x12;
        other[1] = 0x34;
        let text = describe(&other);
        assert!(text.starts_with("unknown variant"), "{text}");
        assert!(text.contains("1234"), "{text}");
    }

    #[test]
    fn parses_a_board_definition() {
        let board = parse(
            "name = Test\n\
             compatible = vendor,test\n\
             bl2 = b04b7abaad152f5708d8aff216a0765b5bef4f726beeda1c5f199376c8434934\n\
             bl31 = 1F6444E2244FD19AF655132AAAA1758A81798DD5C851FBE46E85567F915C7DB9\n",
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
    }

    #[test]
    fn rejects_unknown_keys_and_bad_digests() {
        assert!(parse("name = A\nbogus = B\ncompatible = a,b\nbl2 = 00\nbl31 = 11\n").is_err());
        assert!(parse("name = A\ncompatible = a,b\nbl2 = 00\nbl31 = 11\n").is_err());
        assert!(parse("name = A\ncompatible = a,b\nbl2 = nothex\nbl31 = 11\n").is_err());
        assert!(parse("name = A\ncompatible = a,b\nbl2 = b04b\nbl31 = 11\n").is_err());
    }
}
