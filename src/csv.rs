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

/// 检测分隔符：取前 20 行作为样本，对每个候选做引号感知的解析，选「每行列数
/// 最一致、且能切出两列以上」的那个；完全平局时按 `CANDIDATES` 的顺序。
///
/// 只看引号外的分隔符出现次数是不够的：`id,notes / 1,"a;b;c"` 这种第 2 列是
/// 引号字段的文件，分号候选会把 `"a;b;c"` 当成三个字段（它的引号不在该候选的
/// 字段开头），次数反而更多。按行结构评分才不会被引号里的分隔符骗到。
pub fn detect_delimiter(text: &str) -> u8 {
    let (sample, truncated) = head_sample(text, 20);
    let mut best = CANDIDATES[0];
    let mut best_score = 0u32;
    for &candidate in CANDIDATES.iter() {
        let score = shape_score(sample, candidate, truncated);
        if score > best_score {
            best_score = score;
            best = candidate;
        }
    }
    best
}

/// 样本前 `max_lines` 个物理行；第二个返回值表示是否在样本中途截断。
fn head_sample(text: &str, max_lines: usize) -> (&str, bool) {
    let mut lines = 0usize;
    for (i, b) in text.as_bytes().iter().enumerate() {
        if *b != b'\n' {
            continue;
        }
        lines += 1;
        if lines >= max_lines {
            return (&text[..=i], i + 1 < text.len());
        }
    }
    (text, false)
}

/// 候选分隔符的结构评分（0..=1000）：按该分隔符解析样本，取字段数的众数，
/// 得众数占行数的比例。切不出两列及以上、或者它把引号字段切开了，都为 0。
fn shape_score(sample: &str, delimiter: u8, truncated: bool) -> u32 {
    let mut parsed = parse(sample, delimiter);
    if truncated {
        // 采样点可能落在引号里，末行不完整，不参与统计
        parsed.rows.pop();
    }
    let total = parsed.rows.len();
    if total == 0 {
        return 0;
    }
    // 结构再整齐也不作数：这个分隔符是从引号字段中间切过去的
    if has_split_quoted_fields(sample, &parsed) {
        return 0;
    }

    let mut freq: Vec<(usize, usize)> = Vec::new(); // (列数, 出现次数)
    for row in &parsed.rows {
        let columns = row.fields.len();
        match freq.iter_mut().find(|(cols, _)| *cols == columns) {
            Some(entry) => entry.1 += 1,
            None => freq.push((columns, 1)),
        }
    }
    // 次数最多的列数；并列时取较小列数，让更窄的分隔符占优
    let mut mode = (0usize, 0usize);
    for &(columns, times) in &freq {
        if times > mode.1 || (times == mode.1 && columns < mode.0) {
            mode = (columns, times);
        }
    }
    if mode.0 < 2 {
        return 0;
    }
    (mode.1 * 1000 / total) as u32
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

/// 字段只有一侧带引号：说明当前分隔符从中间切开了引号字段（多见于设置里固定了
/// 错误的分隔符）。这时补空白或裁剪都会落到别人的引号里面去，宁可不动。
fn splits_inside_quotes(raw: &str) -> bool {
    let opens = raw.trim_start().starts_with('"');
    let closes = raw.trim_end().ends_with('"');
    opens != closes
}

/// 解析结果里是否存在被分隔符切开的引号字段。改写文档的命令（对齐、裁剪）在
/// 动手前都要先过这一关：分隔符判错时切出来的碎片不能当真。
pub fn has_split_quoted_fields(text: &str, parsed: &Parsed) -> bool {
    parsed.rows.iter().any(|row| {
        row.fields.iter().any(|field| {
            field.end <= text.len() && splits_inside_quotes(&text[field.start..field.end])
        })
    })
}

/// 按列最大宽度对齐。任一字段含换行（多行字段）时返回 None：补空格会破坏结构。
pub fn align(text: &str, parsed: &Parsed) -> Option<String> {
    if parsed.rows.is_empty() {
        return None;
    }
    if has_split_quoted_fields(text, parsed) {
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
