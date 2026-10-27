//! `read` keeps HTML tables as tables (#781, G6).
//!
//! The text converter in `read.rs` has no DOM: it drops `<td>`/`<th>` tags, so
//! a price table came out as `Widget A123.25`. Before that pass runs, every
//! innermost `<table>` with two or more columns is replaced by plain text:
//!
//! - a Markdown table (`| Widget A | 12 | 3.25 |`), or
//! - when a cell spans rows (`rowspan`), which Markdown cannot show, one row
//!   per line with the cells separated by tabs.
//!
//! Cell text is the cell's text with whitespace collapsed, nothing else
//! changed. Tables that hold other tables, single-column tables and
//! `role="presentation"` tables are page layout, not data, and are left to the
//! ordinary text pass.

use super::decode_html_entities;

/// Stands in for a tab between cells until the text pass has collapsed
/// whitespace, which would turn a real tab into a space. A private-use
/// character, so it never comes from a page.
pub(super) const CELL_SEP: char = '\u{E000}';

/// Upper bounds, so a hostile `colspan="100000"` cannot blow up the output.
const MAX_COLSPAN: usize = 64;
const MAX_COLUMNS: usize = 256;

/// Replace each data table in `html` with its text rendering. Everything else
/// is returned unchanged.
pub(super) fn render_tables(html: &str) -> String {
    let spans = leaf_table_spans(html);
    if spans.is_empty() {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    for (start, end) in spans {
        out.push_str(&html[cursor..start]);
        match render_table(&html[start..end]) {
            Some(text) => {
                out.push_str("\n\n");
                out.push_str(&text);
                out.push_str("\n\n");
            }
            None => out.push_str(&html[start..end]),
        }
        cursor = end;
    }
    out.push_str(&html[cursor..]);
    out
}

/// One tag found while scanning: its byte range, lowercase name, and whether
/// it is a closing tag.
struct Tag<'a> {
    start: usize,
    end: usize,
    name: String,
    closing: bool,
    raw: &'a str,
}

/// Walk the tags of `html` in order; a comment is one tag named `!--`.
fn tags(html: &str) -> Vec<Tag<'_>> {
    let bytes = html.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while let Some(rel) = html[i..].find('<') {
        let start = i + rel;
        if html[start..].starts_with("<!--") {
            // Recorded so its text never lands in a cell; matches no tag name.
            let end = html[start + 4..]
                .find("-->")
                .map(|r| start + 4 + r + 3)
                .unwrap_or(html.len());
            found.push(Tag {
                start,
                end,
                name: "!--".to_string(),
                closing: false,
                raw: &html[start..end],
            });
            i = end;
            continue;
        }
        let end = tag_end(bytes, start);
        let raw = &html[start..end];
        let inner = raw[1..].trim_end_matches('>');
        let closing = inner.starts_with('/');
        let name: String = inner
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            // `a < b`: not a tag.
            i = start + 1;
            continue;
        }
        found.push(Tag {
            start,
            end,
            name,
            closing,
            raw,
        });
        i = end;
    }
    found
}

/// The byte just past the `>` closing the tag that opens at `start`; a `>`
/// inside a quoted attribute value does not count.
fn tag_end(bytes: &[u8], start: usize) -> usize {
    let mut quote: Option<u8> = None;
    let mut i = start + 1;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => return i + 1,
            None => {}
        }
        i += 1;
    }
    bytes.len()
}

/// Byte ranges of the innermost tables: closed `<table>` elements with no
/// table inside. Sorted and never overlapping.
fn leaf_table_spans(html: &str) -> Vec<(usize, usize)> {
    // (start of `<table`, contains another table)
    let mut stack: Vec<(usize, bool)> = Vec::new();
    let mut spans = Vec::new();
    for tag in tags(html) {
        if tag.name != "table" {
            continue;
        }
        if !tag.closing {
            if let Some(parent) = stack.last_mut() {
                parent.1 = true;
            }
            stack.push((tag.start, false));
        } else if let Some((start, has_child)) = stack.pop() {
            if !has_child {
                spans.push((start, tag.end));
            }
        }
    }
    spans.sort_unstable();
    spans
}

#[derive(Default)]
struct Cell {
    text: String,
    header: bool,
    colspan: usize,
    rowspan: usize,
}

#[derive(Default)]
struct Row {
    cells: Vec<Cell>,
    in_thead: bool,
}

/// Render one `<table>…</table>` (no nested tables) as text, or `None` when it
/// is layout rather than data.
fn render_table(fragment: &str) -> Option<String> {
    let tags = tags(fragment);
    let open = tags.first()?;
    if attr(open.raw, "role").is_some_and(|role| role.eq_ignore_ascii_case("presentation"))
        || attr(open.raw, "role").is_some_and(|role| role.eq_ignore_ascii_case("none"))
    {
        return None;
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut row: Option<Row> = None;
    let mut cell: Option<Cell> = None;
    let mut caption: Option<String> = None;
    let mut in_caption = false;
    let mut in_thead = false;

    fn finish_cell(cell: &mut Option<Cell>, row: &mut Option<Row>, in_thead: bool) {
        if let Some(done) = cell.take() {
            row.get_or_insert_with(|| Row {
                cells: Vec::new(),
                in_thead,
            })
            .cells
            .push(done);
        }
    }
    fn finish_row(row: &mut Option<Row>, rows: &mut Vec<Row>) {
        if let Some(done) = row.take() {
            if !done.cells.is_empty() {
                rows.push(done);
            }
        }
    }

    let mut cursor = open.end;
    for tag in tags.iter().skip(1) {
        let text = &fragment[cursor..tag.start];
        if let Some(open_cell) = cell.as_mut() {
            open_cell.text.push_str(text);
        } else if in_caption {
            caption.get_or_insert_with(String::new).push_str(text);
        }
        cursor = tag.end;

        match (tag.name.as_str(), tag.closing) {
            ("tr", false) => {
                finish_cell(&mut cell, &mut row, in_thead);
                finish_row(&mut row, &mut rows);
                row = Some(Row {
                    cells: Vec::new(),
                    in_thead,
                });
            }
            ("tr", true) => {
                finish_cell(&mut cell, &mut row, in_thead);
                finish_row(&mut row, &mut rows);
            }
            ("td" | "th", false) => {
                finish_cell(&mut cell, &mut row, in_thead);
                cell = Some(Cell {
                    text: String::new(),
                    header: tag.name == "th",
                    colspan: span_attr(tag.raw, "colspan"),
                    rowspan: span_attr(tag.raw, "rowspan"),
                });
            }
            ("td" | "th", true) => finish_cell(&mut cell, &mut row, in_thead),
            ("thead", false) => in_thead = true,
            ("thead", true) | ("tbody" | "tfoot", _) => {
                finish_cell(&mut cell, &mut row, in_thead);
                finish_row(&mut row, &mut rows);
                in_thead = false;
            }
            ("caption", false) => in_caption = true,
            ("caption", true) => in_caption = false,
            ("table", true) => {
                finish_cell(&mut cell, &mut row, in_thead);
                finish_row(&mut row, &mut rows);
            }
            // A line break or block inside a cell separates words.
            (
                "br" | "p" | "div" | "li" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                | "section" | "article" | "blockquote" | "pre",
                _,
            ) => {
                if let Some(open_cell) = cell.as_mut() {
                    open_cell.text.push(' ');
                }
            }
            _ => {}
        }
    }
    finish_cell(&mut cell, &mut row, in_thead);
    finish_row(&mut row, &mut rows);

    let columns = rows
        .iter()
        .map(|r| r.cells.iter().map(|c| c.colspan).sum::<usize>())
        .max()
        .unwrap_or(0)
        .min(MAX_COLUMNS);
    if columns < 2 {
        return None;
    }

    let caption = caption.map(|c| cell_text(&c)).filter(|c| !c.is_empty());
    let spans_rows = rows.iter().any(|r| r.cells.iter().any(|c| c.rowspan > 1));

    let mut out = String::new();
    if let Some(caption) = caption {
        out.push_str(&encode(&caption));
        out.push_str("\n\n");
    }
    if spans_rows {
        render_tab_separated(&rows, &mut out);
    } else {
        render_markdown(&rows, columns, &mut out);
    }
    Some(out)
}

/// A Markdown table. The first row is the header when it is a row of `<th>`
/// cells or sits in `<thead>`; otherwise the header row is left blank, since
/// Markdown needs one and inventing names would change the data.
fn render_markdown(rows: &[Row], columns: usize, out: &mut String) {
    let is_header = |row: &Row| row.in_thead || row.cells.iter().all(|cell| cell.header);
    let (header, body) = match rows.split_first() {
        Some((first, rest)) if is_header(first) => (Some(first), rest),
        _ => (None, rows),
    };
    let line = |row: Option<&Row>| -> String {
        let mut cells: Vec<String> = Vec::with_capacity(columns);
        if let Some(row) = row {
            for cell in &row.cells {
                cells.push(encode(&cell_text(&cell.text).replace('|', "\\|")));
                // A cell spanning columns keeps the columns lined up with
                // empty cells after it.
                for _ in 1..cell.colspan {
                    cells.push(String::new());
                }
            }
        }
        cells.truncate(columns);
        cells.resize(columns, String::new());
        format!("| {} |", cells.join(" | "))
    };
    out.push_str(&line(header));
    out.push('\n');
    out.push_str(&format!("|{}", " --- |".repeat(columns)));
    for row in body {
        out.push('\n');
        out.push_str(&line(Some(row)));
    }
}

/// One row per line, cells separated by tabs, in source order.
fn render_tab_separated(rows: &[Row], out: &mut String) {
    let lines: Vec<String> = rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| encode(&cell_text(&cell.text)))
                .collect::<Vec<_>>()
                .join(&CELL_SEP.to_string())
        })
        .collect();
    out.push_str(&lines.join("\n"));
}

/// A cell's text: tags already gone, entities decoded, whitespace collapsed.
fn cell_text(raw: &str) -> String {
    decode_html_entities(raw)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Re-escape `&` and `<`, because the rendered table goes back into the HTML
/// stream: the text pass must not read `<` as a tag, and it decodes entities
/// once more at the end.
fn encode(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;")
}

/// A `colspan`/`rowspan` value, 1 when absent or invalid.
fn span_attr(tag: &str, name: &str) -> usize {
    attr(tag, name)
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(1)
        .min(MAX_COLSPAN)
}

/// The value of attribute `name` in the raw tag text, if present.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(name) {
        let start = from + rel;
        let after = start + name.len();
        from = after;
        let preceded = start > 0 && bytes[start - 1].is_ascii_whitespace();
        if !preceded {
            continue;
        }
        let rest = &lower[after..];
        let trimmed = rest.trim_start();
        let Some(value_part) = trimmed.strip_prefix('=') else {
            continue;
        };
        let value_part = value_part.trim_start();
        let value_start = tag.len() - value_part.len();
        let (value_start, value_end) = match value_part.as_bytes().first() {
            Some(q @ (b'"' | b'\'')) => {
                let close = value_part[1..].find(*q as char)?;
                (value_start + 1, value_start + 1 + close)
            }
            _ => {
                let len = value_part
                    .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                    .unwrap_or(value_part.len());
                (value_start, value_start + len)
            }
        };
        return Some(&tag[value_start..value_end]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::html_to_markdownish;

    /// The page the gap analysis found the bug on (#781 G6).
    const PRICE_HTML: &str = include_str!("../native/e2e_fixtures/price_table.html");

    #[test]
    fn the_price_table_reads_as_a_markdown_table() {
        let text = html_to_markdownish(PRICE_HTML);
        assert!(
            text.contains(
                "| Item | Qty | Unit |\n| --- | --- | --- |\n| Widget A | 12 | 3.25 |\n\
                 | Widget B | 5 | 7.10 |\n| Widget C | 3 | 11.00 |"
            ),
            "{text}"
        );
        assert!(!text.contains("Widget A123.25"), "{text}");
        assert!(text.contains("# Supplier quote #Q-4471"), "{text}");
        assert!(text.contains("Quote valid until 2026-11-30."), "{text}");
    }

    #[test]
    fn cell_text_is_kept_exactly_with_pipes_escaped() {
        let html = "<table><thead><tr><th>Name</th><th>Note</th></tr></thead>\
            <tbody><tr><td> A &amp; B </td><td>x | y &lt;z&gt;</td></tr>\
            <tr><td>line<br>two</td><td><b>bold</b>er</td></tr></tbody></table>";
        let text = html_to_markdownish(html);
        assert!(
            text.contains(
                "| Name | Note |\n| --- | --- |\n| A & B | x \\| y <z> |\n| line two | bolder |"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_table_without_header_cells_gets_a_blank_header() {
        let html = "<table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>";
        let text = html_to_markdownish(html);
        assert_eq!(text, "| | |\n| --- | --- |\n| a | b |\n| c | d |");
    }

    #[test]
    fn colspan_is_padded_and_rowspan_falls_back_to_tabs() {
        let html = "<table><tr><th>Q</th><th>Total</th><th>Note</th></tr>\
            <tr><td colspan=\"2\">sum</td><td>n</td></tr></table>";
        let text = html_to_markdownish(html);
        assert!(text.contains("| sum | | n |"), "{text}");

        let html = "<table><tr><th>Region</th><th>Month</th><th>Sales</th></tr>\
            <tr><td rowspan=2>EU</td><td>Jan</td><td>10</td></tr>\
            <tr><td>Feb</td><td>12</td></tr></table>";
        let text = html_to_markdownish(html);
        assert_eq!(text, "Region\tMonth\tSales\nEU\tJan\t10\nFeb\t12");
    }

    #[test]
    fn layout_tables_are_left_to_the_text_pass() {
        // One column: layout.
        let html = "<table><tr><td>Only</td></tr><tr><td>column</td></tr></table>";
        assert_eq!(html_to_markdownish(html), "Only\n\ncolumn");
        // role=presentation: layout.
        let html = "<table role=\"presentation\"><tr><td>a</td><td>b</td></tr></table>";
        assert_eq!(html_to_markdownish(html), "ab");
        // A table holding a table: only the inner one is data.
        let html = "<table><tr><td>outer</td><td><table><tr><td>1</td><td>2</td></tr>\
            </table></td></tr></table>";
        let text = html_to_markdownish(html);
        assert!(text.contains("outer"), "{text}");
        assert!(text.contains("| 1 | 2 |"), "{text}");
    }

    #[test]
    fn captions_and_comments_and_quoted_brackets() {
        let html = "<table><caption>Prices</caption><!-- <table> -->\
            <tr><th title=\"a>b\">K</th><th>V</th></tr><tr><td>k</td><td>v</td></tr></table>";
        let text = html_to_markdownish(html);
        assert!(
            text.contains("Prices\n\n| K | V |\n| --- | --- |\n| k | v |"),
            "{text}"
        );
    }

    #[test]
    fn hostile_colspan_is_bounded() {
        let html = "<table><tr><td colspan=999999>a</td><td>b</td></tr></table>";
        let text = html_to_markdownish(html);
        assert!(text.lines().all(|line| line.len() < 1000), "{}", text.len());
    }

    #[test]
    fn an_unclosed_table_is_left_alone() {
        let html = "<p>x</p><table><tr><td>a</td><td>b</td>";
        assert_eq!(html_to_markdownish(html), "x\n\nab");
    }
}
