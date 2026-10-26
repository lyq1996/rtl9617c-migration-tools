// SPDX-License-Identifier: GPL-2.0-only

use std::sync::OnceLock;

use crate::embedded::BOARD_SOURCES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub name: String,
    pub compatible: String,
    pub volumes: Vec<String>,
}

fn parse(source: &str) -> Result<Board, String> {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut pending = String::new();

    for (number, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let joined = if let Some(head) = line.strip_suffix('\\') {
            pending.push_str(head.trim());
            pending.push(' ');
            continue;
        } else {
            let mut complete = std::mem::take(&mut pending);
            complete.push_str(line);
            complete
        };
        let Some((key, value)) = joined.split_once('=') else {
            return Err(format!("line {}: expected `key = value`", number + 1));
        };
        fields.push((key.trim().to_string(), value.trim().to_string()));
    }
    if !pending.is_empty() {
        return Err("the last value ends with a continuation".to_string());
    }

    let take = |wanted: &str| -> Result<String, String> {
        fields
            .iter()
            .find(|(key, _)| key == wanted)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| format!("`{wanted}` is missing"))
    };

    for (key, _) in &fields {
        if !matches!(key.as_str(), "name" | "compatible" | "volumes") {
            return Err(format!("unknown key `{key}`"));
        }
    }

    let volumes: Vec<String> = take("volumes")?
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    if volumes.is_empty() {
        return Err("`volumes` is empty".to_string());
    }

    Ok(Board {
        name: take("name")?,
        compatible: take("compatible")?,
        volumes,
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

pub fn identify(volumes: &[String]) -> Result<&'static Board, String> {
    if let Some(board) = all().iter().find(|board| board.volumes == volumes) {
        return Ok(board);
    }

    let expected = all()
        .iter()
        .map(|board| format!("{}: {}", board.name, board.volumes.join(", ")))
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "This unit's UBI volume table is not one this tool knows ({}). Known tables - {expected}",
        volumes.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_configured_board_parses() {
        let boards = all();
        assert!(!boards.is_empty(), "no board definitions were embedded");
        for board in boards {
            assert!(!board.name.is_empty());
            assert!(board.compatible.contains(','));
            assert!(board.volumes.iter().all(|name| name.starts_with("ubi_")));
        }
    }

    #[test]
    fn names_and_compatible_entries_are_unique() {
        let boards = all();
        for (index, board) in boards.iter().enumerate() {
            for other in &boards[index + 1..] {
                assert_ne!(board.name, other.name);
                assert_ne!(board.compatible, other.compatible);
                assert_ne!(board.volumes, other.volumes);
            }
        }
    }

    #[test]
    fn identifies_every_configured_board_from_its_own_table() {
        for board in all() {
            let identified = identify(&board.volumes).unwrap();
            assert_eq!(identified.name, board.name);
        }
    }

    #[test]
    fn refuses_a_table_it_does_not_recognise() {
        let error = identify(&["ubi_Config".to_string(), "ubi_nope".to_string()]).unwrap_err();
        assert!(error.contains("not one this tool knows"), "{error}");
    }

    #[test]
    fn parses_comments_and_continuations() {
        let board = parse(
            "# a board\n\
             name = Test\n\
             compatible = vendor,test\n\
             volumes = ubi_Config, \\\n\
             ubi_apps\n",
        )
        .unwrap();
        assert_eq!(board.name, "Test");
        assert_eq!(board.volumes, vec!["ubi_Config", "ubi_apps"]);
    }

    #[test]
    fn rejects_unknown_keys_and_missing_values() {
        assert!(parse("name = A\nbogus = B\nvolumes = ubi_a\n").is_err());
        assert!(parse("name = A\nvolumes = ubi_a\n").is_err());
        assert!(parse("name = A\ncompatible = a,b\nvolumes = \n").is_err());
    }
}
