pub fn format_size(size: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;

    if size >= MIB {
        if size.is_multiple_of(MIB) {
            format!("{} MiB", size / MIB)
        } else {
            format!("{:.2} MiB", size as f64 / MIB as f64)
        }
    } else if size >= KIB {
        if size.is_multiple_of(KIB) {
            format!("{} KiB", size / KIB)
        } else {
            format!("{:.2} KiB", size as f64 / KIB as f64)
        }
    } else {
        format!("{size} B")
    }
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn formats_exact_binary_units() {
        assert_eq!(format_size(0x40000), "256 KiB");
        assert_eq!(format_size(0x200000), "2 MiB");
        assert_eq!(format_size(0xf5c0000), "245.75 MiB");
    }

    #[test]
    fn formats_partial_units() {
        assert_eq!(format_size(0x1f000), "124 KiB");
        assert_eq!(format_size(0x60e000), "6.05 MiB");
        assert_eq!(format_size(900), "900 B");
    }
}
