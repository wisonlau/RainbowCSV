//! CSV/TSV 解析：分隔符检测、字段切分、对齐与裁剪。
//!
//! 全部按 UTF-8 字节偏移工作（与 `syntax-provider` 的约定一致），只对
//! ASCII 的分隔符、引号、换行做状态判断，所以多字节字符不会被切断。

/// 自动检测时依次尝试的分隔符，平局时靠前的优先。
pub const CANDIDATES: [u8; 4] = [b',', b';', b'\t', b'|'];

/// 一个字段：`start..end` 是原始字节范围，含首尾空白与引号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub start: usize,
    pub end: usize,
}

impl Field {
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// 一行：`fields` 按出现顺序；`start..end` 覆盖整行（不含换行符）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub fields: Vec<Field>,
    pub start: usize,
    pub end: usize,
}

/// 解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub delimiter: u8,
    pub rows: Vec<Row>,
}

/// 分隔符的显示名，用于提示信息。
pub fn delimiter_name(delimiter: u8) -> &'static str {
    match delimiter {
        b',' => "comma (,)",
        b';' => "semicolon (;)",
        b'\t' => "tab",
        b'|' => "pipe (|)",
        _ => "unknown",
    }
}

fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// 前 `field_start..i` 全是空白时返回 true，用于判断引号是否是字段的开头。
fn blank_before(bytes: &[u8], field_start: usize, i: usize) -> bool {
    bytes[field_start..i].iter().all(|b| is_blank(*b) || *b == b'\r')
}

/// 按 RFC 4180 切分：引号内的分隔符与换行都属于同一个字段，`""` 是转义引号。
pub fn parse(text: &str, delimiter: u8) -> Parsed {
    let bytes = text.as_bytes();
    let mut rows = Vec::new();
    let mut fields: Vec<Field> = Vec::new();
    let mut field_start = 0usize;
    let mut row_start = 0usize;
    let mut in_quotes = false;
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];

        if in_quotes {
            if b == b'"' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                    i += 2; // 转义的双引号
                    continue;
                }
                in_quotes = false;
            }
            i += 1;
            continue;
        }

        if b == delimiter {
            fields.push(Field { start: field_start, end: i });
            field_start = i + 1;
            i += 1;
            continue;
        }

        if b == b'\n' {
            let end = if i > field_start && bytes[i - 1] == b'\r' { i - 1 } else { i };
            fields.push(Field { start: field_start, end });
            if end > row_start {
                rows.push(Row { fields: std::mem::take(&mut fields), start: row_start, end });
            } else {
                fields.clear();
            }
            row_start = i + 1;
            field_start = i + 1;
            i += 1;
            continue;
        }

        if b == b'"' && blank_before(bytes, field_start, i) {
            in_quotes = true;
            i += 1;
            continue;
        }

        i += 1;
    }

    // 收尾：最后一行没有换行符
    if field_start < bytes.len() || !fields.is_empty() {
        let end = bytes.len();
        if end > field_start || !fields.is_empty() {
            fields.push(Field { start: field_start, end });
            if end > row_start {
                rows.push(Row { fields, start: row_start, end });
            }
        }
    }

    Parsed { delimiter, rows }
}

/// 字段里实际要着色的范围：剥掉首尾空白，引号保留（和 JetBrains 里的观感一致）。
pub fn content_range(text: &str, field: Field) -> Option<(usize, usize)> {
    if field.is_empty() || field.end > text.len() {
        return None;
    }
    let raw = &text[field.start..field.end];
    let lead = raw.len() - raw.trim_start().len();
    let core = raw.trim_end();
    if lead >= core.len() {
        return None; // 纯空白字段不着色，避免零长度区间
    }
    Some((field.start + lead, field.start + core.len()))
}

/// 引号字段的内部范围（不含引号），用于裁剪内容。
fn inner_range(text: &str, field: Field) -> Option<(usize, usize)> {
    let (start, end) = content_range(text, field)?;
    let core = &text[start..end];
    if core.len() >= 2 && core.starts_with('"') && core.ends_with('"') {
        Some((start + 1, end - 1))
    } else {
        None
    }
}

/// 检测分隔符：取前 20 个非空行，选平均切分次数最多的候选。
///
/// 引号内的分隔符不计入，避免 `"a,b";c` 这类行把逗号误判成主分隔符。
pub fn detect_delimiter(text: &str) -> u8 {
    let bytes = text.as_bytes();
    let mut counts = [0usize; CANDIDATES.len()];
    let mut lines = 0usize;
    let mut i = 0usize;
    let mut row_start = 0usize;
    let mut in_quotes = false;

    while i < bytes.len() && lines < 20 {
        let b = bytes[i];
        if in_quotes {
            if b == b'"' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                    i += 2;
                    continue;
                }
                in_quotes = false;
            }
            i += 1;
            continue;
        }
        if b == b'"' && blank_before(bytes, row_start, i) {
            in_quotes = true;
            i += 1;
            continue;
        }
        if b == b'\n' {
            let blank = bytes[row_start..i]
                .iter()
                .all(|c| matches!(c, b'\r' | b' ' | b'\t'));
            if !blank {
                lines += 1;
            }
            row_start = i + 1;
            i += 1;
            continue;
        }
        if let Some(pos) = CANDIDATES.iter().position(|c| *c == b) {
            counts[pos] += 1;
        }
        i += 1;
    }

    let mut best = 0usize;
    for (pos, count) in counts.iter().enumerate() {
        if *count > counts[best] {
            best = pos;
        }
    }
    CANDIDATES[best]
}

/// 光标（字节偏移）所在的 (行, 列)。
pub fn field_at(parsed: &Parsed, offset: usize) -> Option<(usize, usize)> {
    for (ri, row) in parsed.rows.iter().enumerate() {
        if offset < row.start || offset > row.end {
            continue;
        }
        for (ci, field) in row.fields.iter().enumerate() {
            if offset >= field.start && offset <= field.end {
                return Some((ri, ci));
            }
        }
        // 落在行尾之后（比如行末的分隔符后）时归到最后一列
        if let Some(last) = row.fields.len().checked_sub(1) {
            return Some((ri, last));
        }
    }
    None
}

/// 同一列的所有字段范围。
pub fn column_fields(parsed: &Parsed, column: usize) -> impl Iterator<Item = (usize, Field)> + '_ {
    parsed
        .rows
        .iter()
        .enumerate()
        .filter_map(move |(ri, row)| row.fields.get(column).map(|f| (ri, *f)))
}

/// 从 `offset` 往后找下一个字段；已在最后一个字段时返回 None。
pub fn next_field(parsed: &Parsed, offset: usize) -> Option<Field> {
    for row in &parsed.rows {
        for field in &row.fields {
            if field.start > offset {
                return Some(*field);
            }
        }
    }
    None
}

/// 从 `offset` 往前找上一个字段；字段内会先退到本字段开头。
pub fn prev_field(parsed: &Parsed, offset: usize) -> Option<Field> {
    let mut best = None;
    for row in &parsed.rows {
        for field in &row.fields {
            if field.start < offset {
                best = Some(*field);
            } else {
                return best;
            }
        }
    }
    best
}

/// 按列最大宽度对齐。任一字段含换行（多行字段）时返回 None：补空格会破坏结构。
pub fn align(text: &str, parsed: &Parsed) -> Option<String> {
    if parsed.rows.is_empty() {
        return None;
    }

    // 宽度按字符数算，中文等宽字符会略窄，但比字节数靠谱
    let mut widths: Vec<usize> = Vec::new();
    for row in &parsed.rows {
        for (ci, field) in row.fields.iter().enumerate() {
            let raw = &text[field.start..field.end];
            if raw.contains('\n') {
                return None;
            }
            let width = raw.chars().count();
            if ci >= widths.len() {
                widths.push(width);
            } else if width > widths[ci] {
                widths[ci] = width;
            }
        }
    }

    let uses_crlf = text.contains("\r\n");
    let newline = if uses_crlf { "\r\n" } else { "\n" };
    let delimiter = parsed.delimiter as char;
    let mut out = String::with_capacity(text.len() + parsed.rows.len() * 8);

    for (ri, row) in parsed.rows.iter().enumerate() {
        if ri > 0 {
            out.push_str(newline);
        }
        for (ci, field) in row.fields.iter().enumerate() {
            if ci > 0 {
                out.push(delimiter);
            }
            let raw = &text[field.start..field.end];
            out.push_str(raw);
            // 最后一列不补空格，避免行尾空白
            if ci + 1 < row.fields.len() {
                let pad = widths.get(ci).copied().unwrap_or(0).saturating_sub(raw.chars().count());
                for _ in 0..pad {
                    out.push(' ');
                }
            }
        }
    }

    // 原文以换行结尾时补回，避免对齐后少一行
    if text.ends_with('\n') {
        out.push_str(newline);
    }

    Some(out)
}

/// 去掉每个字段首尾的空白；引号字段只裁引号内部，引号本身保留。
pub fn trim_cells(text: &str, parsed: &Parsed) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;

    for row in &parsed.rows {
        for field in &row.fields {
            if field.start < cursor || field.end > text.len() {
                continue;
            }
            out.push_str(&text[cursor..field.start]);
            out.push_str(&trimmed_field(text, *field));
            cursor = field.end;
        }
    }
    if cursor <= text.len() {
        out.push_str(&text[cursor..]);
    }
    out
}

fn trimmed_field(text: &str, field: Field) -> String {
    let Some((start, end)) = content_range(text, field) else {
        return String::new();
    };
    match inner_range(text, field) {
        Some((inner_start, inner_end)) => {
            let inner = &text[inner_start..inner_end];
            format!("\"{}\"", inner.trim())
        }
        None => text[start..end].trim().to_string(),
    }
}

/// 统计列数（取所有行的最大值），用于提示信息。
pub fn column_count(parsed: &Parsed) -> usize {
    parsed.rows.iter().map(|r| r.fields.len()).max().unwrap_or(0)
}
