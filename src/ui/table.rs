use crate::Result;
use comfy_table::Table;

/// Print a comfy-table, removing leading/trailing row padding.
///
/// comfy-table's default cell padding is (1, 1), which adds a leading and
/// trailing space to each row. With the `NOTHING` preset (no borders),
/// these spaces are the only indentation.
///
/// `str::trim` doesn't work on rows with ANSI color codes (from `Cell::fg`)
/// because the trailing padding sits inside the color/reset sequence and
/// isn't recognized as trailing whitespace.
pub fn print_table(table: Table) -> Result<()> {
    let table = table.to_string();
    for line in table.lines() {
        println!("{}", trim_ansi_line(line));
    }
    Ok(())
}

/// Trim leading whitespace, then trim trailing whitespace that may be
/// enclosed within ANSI escape sequences.
fn trim_ansi_line(line: &str) -> String {
    let s = line.trim_start();
    if s.is_empty() {
        return String::new();
    }
    // Strip ANSI codes to find the visible end position, then cut the
    // original string just past the last non-whitespace visible char
    // (keeping any ANSI reset that precedes the trailing padding).
    let plain = console::strip_ansi_codes(s);
    let visible_end = plain.trim_end().chars().count();

    // Walk the original string, counting visible characters, and stop
    // once we've emitted `visible_end` visible chars. ANSI escape sequences
    // are passed through without counting toward visible length.
    //
    // CSI sequences (\x1b[...X) terminate on any byte in 0x40..=0x7E.
    // Other escape sequences (\x1b followed by a single byte) terminate
    // after one byte.
    let mut result = String::with_capacity(s.len());
    let mut visible = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            result.push(ch);
            // Check for CSI: \x1b[
            if matches!(chars.peek(), Some('[')) {
                result.push(chars.next().unwrap());
                // Consume until we hit a final byte (0x40..=0x7E).
                for c in chars.by_ref() {
                    result.push(c);
                    if c.is_ascii() && (0x40..=0x7e).contains(&(c as u8)) {
                        break;
                    }
                }
            } else {
                // Non-CSI escape: consume one more byte.
                if let Some(c) = chars.next() {
                    result.push(c);
                }
            }
        } else {
            if visible >= visible_end {
                continue;
            }
            result.push(ch);
            visible += 1;
        }
    }
    result
}
