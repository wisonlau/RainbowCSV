//! Rainbow CSV for OpenIDEA.
//!
//! 仿 JetBrains 的 Rainbow CSV（plugin 12896）：按列给 CSV/TSV 上色，
//! 光标所在列整列高亮，外加对齐、裁剪、字段间跳转几个实用命令。
//!
//! 着色走 `syntax-provider`：插件 API 只允许语义高亮类别（keyword、string、
//! number……），色值由配色方案决定，所以「彩虹」是按列循环取一组色相差异较大
//! 的类别，而不是写死 RGB。

mod csv;
mod palette;

use openidea_plugin::host::{setting_bool, setting_number, setting_string};
use openidea_plugin::language_features::{
    Document, Highlight as ColumnHighlight, LanguageFeatures,
};
use openidea_plugin::syntax::{SyntaxProvider, Token};
use openidea_plugin::{
    Error, Invocation, NotificationLevel, Plugin, Position, Range, TextEdit, register_plugin, text,
};

/// 单文档着色 token 上限：超过就停，宁可少染几列也不要拖慢输入。
const MAX_TOKENS: usize = 60_000;
/// 小于此列数的行按普通行处理。
const DEFAULT_COLORS: usize = palette::MAX_COLORS;
const DEFAULT_MAX_SIZE_KB: i64 = 2048;

#[derive(Default)]
pub struct RainbowCsv;

// ---------- 设置 ----------

fn setting_text(key: &str, default: &str) -> String {
    setting_string(key).unwrap_or_else(|| default.to_string())
}

fn setting_int(key: &str, default: i64) -> i64 {
    match setting_number(key) {
        Some(v) if v.is_finite() => v as i64,
        _ => default,
    }
}

/// 用户固定指定的分隔符；`auto` 返回 None，交给检测。
fn fixed_delimiter() -> Option<u8> {
    match setting_text("delimiter", "auto").as_str() {
        "," => Some(b','),
        ";" => Some(b';'),
        "|" => Some(b'|'),
        "tab" => Some(b'\t'),
        _ => None,
    }
}

fn resolve_delimiter(text: &str) -> u8 {
    fixed_delimiter().unwrap_or_else(|| csv::detect_delimiter(text))
}

/// 超过大小上限时不着色（0 表示不限制）。
fn within_size_limit(text: &str) -> bool {
    let limit = setting_int("maxSizeKb", DEFAULT_MAX_SIZE_KB);
    limit <= 0 || text.len() <= (limit as usize).saturating_mul(1024)
}

fn color_count() -> usize {
    setting_int("colorCount", DEFAULT_COLORS as i64).clamp(palette::MIN_COLORS as i64, palette::MAX_COLORS as i64) as usize
}

// ---------- 列着色 ----------

impl RainbowCsv {
    fn tokens_for(&self, text: &str) -> Vec<Token> {
        if text.is_empty() || !within_size_limit(text) {
            return Vec::new();
        }

        let parsed = csv::parse(text, resolve_delimiter(text));
        let colors = color_count();

        let mut tokens = Vec::new();
        'rows: for row in parsed.rows.iter() {
            for (ci, field) in row.fields.iter().enumerate() {
                let Some((start, end)) = csv::content_range(text, *field) else {
                    continue;
                };
                // 表头跟着列走：第 0 行的名字和它下面那一列同色。
                tokens.push(Token {
                    range: start..end,
                    kind: palette::kind_for(ci, colors),
                });
                if tokens.len() >= MAX_TOKENS {
                    break 'rows;
                }
            }
        }
        tokens
    }
}

impl SyntaxProvider for RainbowCsv {
    fn highlights(&mut self, _language: &str, text: &str) -> Vec<Token> {
        self.tokens_for(text)
    }
}

// ---------- 光标所在列 ----------

impl LanguageFeatures for RainbowCsv {
    fn document_highlights(&mut self, doc: &Document, at: Position) -> Vec<ColumnHighlight> {
        if !setting_bool("highlightColumn").unwrap_or(true) || !within_size_limit(&doc.text) {
            return Vec::new();
        }

        let parsed = csv::parse(&doc.text, resolve_delimiter(&doc.text));
        let Some(offset) = text::offset(&doc.text, at) else {
            return Vec::new();
        };
        let Some((row_index, column)) = csv::field_at(&parsed, offset) else {
            return Vec::new();
        };

        let mut out = Vec::new();
        for (ri, field) in csv::column_fields(&parsed, column) {
            let Some((start, end)) = csv::content_range(&doc.text, field) else {
                continue;
            };
            out.push(ColumnHighlight {
                range: Range::new(
                    text::position(&doc.text, start),
                    text::position(&doc.text, end),
                ),
                // 同行用 write（更醒目），同列其余用 text
                kind: if ri == row_index { "write".into() } else { "text".into() },
            });
        }
        out
    }
}

// ---------- 命令 ----------

fn notify(level: NotificationLevel, body: &str) {
    openidea_plugin::host::notify(level, "Rainbow CSV", body);
}

impl RainbowCsv {
    /// 用 `f` 生成的新文本整篇替换，返回是否真的改了。
    fn replace_document(
        editor: &openidea_plugin::TextEditor,
        build: impl FnOnce(&str) -> Result<String, String>,
    ) -> Result<bool, Error> {
        let doc = editor.document()?;
        let new_text = match build(&doc.text) {
            Ok(v) => v,
            Err(message) => {
                notify(NotificationLevel::Warning, &message);
                return Ok(false);
            }
        };
        if new_text == doc.text {
            return Ok(false);
        }
        let whole = Range::new(Position::new(0, 0), text::position(&doc.text, doc.text.len()));
        editor.apply_edits(doc.version, &[TextEdit { range: whole, text: new_text }])?;
        Ok(true)
    }
}

impl Plugin for RainbowCsv {
    fn run_command(
        &mut self,
        id: &str,
        ctx: &Invocation,
        _arg: Option<&str>,
    ) -> Result<Option<String>, Error> {
        let editor = ctx
            .editor()
            .ok_or_else(|| Error::Invalid("no focused editor".into()))?;

        match id {
            "align-columns" => {
                Self::replace_document(&editor, |text| {
                    let parsed = csv::parse(text, resolve_delimiter(text));
                    csv::align(text, &parsed).ok_or_else(|| {
                        "Cannot align: a field spans more than one line, or the delimiter cuts through a quoted field."
                            .to_string()
                    })
                })?;
                Ok(None)
            }

            "trim-cells" => {
                Self::replace_document(&editor, |text| {
                    let parsed = csv::parse(text, resolve_delimiter(text));
                    // 分隔符判错时切出来的是引号碎片，裁剪会把引号里的空白当边界空白删掉
                    if csv::has_split_quoted_fields(text, &parsed) {
                        return Err(
                            "Cannot trim: the delimiter cuts through a quoted field.".to_string()
                        );
                    }
                    Ok(csv::trim_cells(text, &parsed))
                })?;
                Ok(None)
            }

            "next-field" | "prev-field" => {
                let doc = editor.document()?;
                let parsed = csv::parse(&doc.text, resolve_delimiter(&doc.text));
                let forward = id == "next-field";

                let mut targets = Vec::new();
                for sel in editor.selections()? {
                    let Some(offset) = text::offset(&doc.text, sel.start) else {
                        continue;
                    };
                    let field = if forward {
                        csv::next_field(&parsed, offset)
                    } else {
                        csv::prev_field(&parsed, offset)
                    };
                    let Some(field) = field else { continue };
                    targets.push(Range::new(
                        text::position(&doc.text, field.start),
                        text::position(&doc.text, field.end),
                    ));
                }

                if targets.is_empty() {
                    return Ok(None);
                }
                editor.set_selections(&targets)?;
                editor.reveal(targets[0])?;
                Ok(None)
            }

            "show-delimiter" => {
                let doc = editor.document()?;
                let delimiter = resolve_delimiter(&doc.text);
                let parsed = csv::parse(&doc.text, delimiter);
                let body = format!(
                    "Delimiter: {} — {} rows, {} columns",
                    csv::delimiter_name(delimiter),
                    parsed.rows.len(),
                    csv::column_count(&parsed)
                );
                notify(NotificationLevel::Info, &body);
                Ok(Some(body))
            }

            _ => Err(Error::NotFound(format!("command {id}"))),
        }
    }
}

register_plugin!(RainbowCsv, syntax, language_features);

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::mock::MockHost;
    use openidea_plugin::syntax::Highlight as SyntaxKind;

    fn tokens(text: &str) -> Vec<Token> {
        let _host = MockHost::install();
        RainbowCsv.tokens_for(text)
    }

    fn kinds(text: &str) -> Vec<SyntaxKind> {
        tokens(text).into_iter().map(|t| t.kind).collect()
    }

    fn slice<'a>(text: &'a str, token: &Token) -> &'a str {
        &text[token.range.clone()]
    }

    #[test]
    fn header_row_shares_its_columns_colour() {
        let text = "a,b,c\n1,2,3\n";
        let toks = tokens(text);
        assert_eq!(toks.len(), 6);
        let row: Vec<SyntaxKind> = (0..3).map(|c| palette::kind_for(c, 8)).collect();
        // 表头三格和它下面的数据行用完全相同的列色
        assert_eq!(kinds(text)[..3], row[..]);
        assert_eq!(kinds(text)[3..], row[..]);
        assert_eq!(slice(text, &toks[5]), "3");
    }

    #[test]
    fn same_column_keeps_its_colour_down_the_file() {
        let text = "h1,h2,h3\na,b,c\nd,e,f\n";
        let toks = tokens(text);
        // 表头也算这一列的一部分
        let first = palette::kind_for(0, 8);
        assert_eq!(toks[0].kind, first, "表头首格");
        assert_eq!(toks[3].kind, first, "第二行首格");
        assert_eq!(toks[6].kind, first, "第三行首格");
        assert_ne!(toks[3].kind, toks[4].kind, "相邻列不同色");
    }

    #[test]
    fn quoted_delimiter_does_not_split() {
        let text = "\"a,b\",c\n";
        let toks = tokens(text);
        assert_eq!(toks.len(), 2);
        assert_eq!(slice(text, &toks[0]), "\"a,b\"");
        assert_eq!(slice(text, &toks[1]), "c");
    }

    #[test]
    fn empty_cells_are_not_coloured() {
        let text = "a,,c\n";
        let toks = tokens(text);
        assert_eq!(toks.len(), 2);
        assert_eq!(slice(text, &toks[0]), "a");
        assert_eq!(slice(text, &toks[1]), "c");
    }

    #[test]
    fn last_line_without_newline_is_coloured() {
        let text = "a,b";
        let toks = tokens(text);
        assert_eq!(toks.len(), 2);
        assert_eq!(slice(text, &toks[1]), "b");
    }

    #[test]
    fn blank_and_empty_text_produce_nothing() {
        assert!(tokens("").is_empty());
        assert!(tokens("\n\n\n").is_empty());
    }

    #[test]
    fn tab_separated_file_is_detected() {
        assert_eq!(csv::detect_delimiter("a\tb\tc\n1\t2\t3\n"), b'\t');
        assert_eq!(csv::detect_delimiter("a;b\n1;2\n"), b';');
        assert_eq!(csv::detect_delimiter("a|b\n1|2\n"), b'|');
        assert_eq!(csv::detect_delimiter("a,b\n1,2\n"), b',');
    }

    #[test]
    fn detect_ignores_delimiters_inside_quotes() {
        // 引号里两个逗号，引号外一个分号：分号才是真分隔符
        assert_eq!(csv::detect_delimiter("\"a,b,c\";d\n"), b';');
    }

    #[test]
    fn detect_ignores_delimiters_in_quoted_fields_after_the_first_column() {
        // 第 2 列整体是引号字段，里面的分号不能算给分号
        let text = "id,notes\n1,\"a;b;c\"\n2,\"long;b;c\"\n";
        assert_eq!(csv::detect_delimiter(text), b',');
    }

    #[test]
    fn detect_keeps_the_real_delimiter_when_the_last_column_is_quoted() {
        let text = "id;notes\n1;\"a,b,c\"\n2;\"long,b,c\"\n";
        assert_eq!(csv::detect_delimiter(text), b';');
    }

    #[test]
    fn align_preserves_quoted_fields_that_contain_the_other_delimiter() {
        let text = "id,notes\n1,\"a;b;c\"\n2,\"long;b;c\"\n";
        let parsed = csv::parse(text, csv::detect_delimiter(text));
        let aligned = csv::align(text, &parsed).expect("align");

        // 对齐后仍是 3 行 2 列，引号里的内容一字不改
        let reparsed = csv::parse(&aligned, b',');
        assert_eq!(reparsed.rows.len(), 3);
        let notes: Vec<&str> = reparsed
            .rows
            .iter()
            .map(|row| {
                let field = row.fields[1];
                &aligned[field.start..field.end]
            })
            .collect();
        assert_eq!(notes, ["notes", "\"a;b;c\"", "\"long;b;c\""]);
    }

    #[test]
    fn align_refuses_when_the_delimiter_splits_inside_quotes() {
        // 设置里固定了错误的分隔符：切分点落在引号里，宁可不动也不改内容
        let text = "id,notes\n1,\"a;b;c\"\n";
        let parsed = csv::parse(text, b';');
        assert!(csv::align(text, &parsed).is_none());
    }

    #[test]
    fn header_follows_its_column() {
        let _host = MockHost::install();
        let toks = RainbowCsv.tokens_for("a,b\n1,2\n");
        assert_eq!(toks[0].kind, toks[2].kind, "第 0 列表头与数据同色");
        assert_eq!(toks[1].kind, toks[3].kind, "第 1 列表头与数据同色");
    }

    #[test]
    fn fixed_delimiter_overrides_detection() {
        let _host = MockHost::install();
        _host.set_setting("delimiter", "\";\"");
        let toks = RainbowCsv.tokens_for("a,b;c\n");
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].range, 0..3);
        assert_eq!(toks[1].range, 4..5);
    }

    #[test]
    fn align_pads_columns() {
        let text = "name,age\nbob,7\nalice,42\n";
        let parsed = csv::parse(text, b',');
        let aligned = csv::align(text, &parsed).unwrap();
        assert_eq!(aligned, "name ,age\nbob  ,7\nalice,42\n");
    }

    #[test]
    fn align_refuses_multiline_fields() {
        let text = "a,\"x\ny\"\n";
        let parsed = csv::parse(text, b',');
        assert!(csv::align(text, &parsed).is_none());
    }

    #[test]
    fn trim_keeps_quotes() {
        let text = "  a  , \" b \" \n";
        let parsed = csv::parse(text, b',');
        assert_eq!(csv::trim_cells(text, &parsed), "a,\"b\"\n");
    }

    #[test]
    fn field_at_finds_caret_column() {
        let text = "a,b,c\n1,2,3\n";
        let parsed = csv::parse(text, b',');
        assert_eq!(csv::field_at(&parsed, 0), Some((0, 0)));
        assert_eq!(csv::field_at(&parsed, 2), Some((0, 1)));
        assert_eq!(csv::field_at(&parsed, 10), Some((1, 2)));
    }

    #[test]
    fn next_and_prev_field_walk_the_grid() {
        let text = "a,b\nc,d\n";
        let parsed = csv::parse(text, b',');
        assert_eq!(csv::next_field(&parsed, 0).map(|f| f.start), Some(2));
        assert_eq!(csv::next_field(&parsed, 2).map(|f| f.start), Some(4));
        assert_eq!(csv::prev_field(&parsed, 4).map(|f| f.start), Some(2));
        assert!(csv::next_field(&parsed, 6).is_none());
    }

    #[test]
    fn document_highlights_marks_the_whole_column() {
        let _host = MockHost::install();
        let doc = Document {
            path: "/p/a.csv".into(),
            language: "csv".into(),
            version: 1,
            text: "h1,h2,h3\na,b,c\nd,e,f\n".into(),
        };
        let highlights = RainbowCsv.document_highlights(&doc, Position::new(1, 2));
        assert_eq!(highlights.len(), 3);
        let cols: Vec<u32> = highlights.iter().map(|h| h.range.start.character).collect();
        assert_eq!(cols, vec![3, 2, 2], "第二个字段的列偏移");
        assert_eq!(highlights[1].kind, "write"); // 光标所在行
        assert_eq!(highlights[0].kind, "text");
    }

    #[test]
    fn sort_of_tokens_is_ascending_and_non_overlapping() {
        let text = "\"x,y\",z\n1,2\n";
        let toks = tokens(text);
        for pair in toks.windows(2) {
            assert!(pair[0].range.end <= pair[1].range.start, "overlap: {:?} {:?}", pair[0], pair[1]);
        }
    }
}


/// 审计：完全按 `plugin.toml` 声明的能力集调用公开入口。
///
/// 对应上游评审的做法——「用清单里确切的能力 + 合成编辑器/光标」跑真实命令，
/// 而不是调用私有辅助函数。这样能力声明和实现对不上时会直接失败。
#[cfg(test)]
mod audit {
    use super::*;
    use openidea_plugin::mock::MockHost;
    use openidea_plugin::syntax::SyntaxProvider;

    /// 从 plugin.toml 读出 `[capabilities] required`，避免测试和清单各自漂移。
    fn declared_capabilities() -> Vec<String> {
        let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/plugin.toml"))
            .expect("plugin.toml should be readable");
        let line = manifest
            .lines()
            .find(|l| l.trim_start().starts_with("required"))
            .expect("capabilities.required should be declared");
        line.split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    fn host_with(caps: &[String]) -> MockHost {
        let refs: Vec<&str> = caps.iter().map(String::as_str).collect();
        MockHost::install().with_capabilities(&refs)
    }

    #[test]
    fn manifest_declares_both_editor_capabilities() {
        let caps = declared_capabilities();
        assert!(caps.iter().any(|c| c == "editor.read"), "缺 editor.read: {caps:?}");
        assert!(caps.iter().any(|c| c == "editor.write"), "缺 editor.write: {caps:?}");
    }

    #[test]
    fn align_command_writes_with_declared_capabilities() {
        let host = host_with(&declared_capabilities());
        host.open_editor("/p/a.csv", "csv", "name,age\nbob,7\nalice,42\n", &[(0, 0, 0, 0)]);
        let out = RainbowCsv.run_command("align-columns", &host.invocation(), None);
        assert!(out.is_ok(), "align 失败: {out:?}");
        assert_eq!(host.editor_text(), "name ,age\nbob  ,7\nalice,42\n");
    }

    #[test]
    fn trim_command_writes_with_declared_capabilities() {
        let host = host_with(&declared_capabilities());
        host.open_editor("/p/a.csv", "csv", "  a  , \" b \" \n", &[(0, 0, 0, 0)]);
        let out = RainbowCsv.run_command("trim-cells", &host.invocation(), None);
        assert!(out.is_ok(), "trim 失败: {out:?}");
        assert_eq!(host.editor_text(), "a,\"b\"\n");
    }

    #[test]
    fn align_is_denied_without_editor_write() {
        // 反向对照：只给 editor.read 时，写文档必须被拒。
        let host = host_with(&["editor.read".to_string()]);
        host.open_editor("/p/a.csv", "csv", "a,bb\nccc,d\n", &[(0, 0, 0, 0)]);
        match RainbowCsv.run_command("align-columns", &host.invocation(), None) {
            Err(Error::Denied(cap)) => assert_eq!(cap, "editor.write"),
            other => panic!("应被拒绝，实际: {other:?}"),
        }
        assert_eq!(host.editor_text(), "a,bb\nccc,d\n", "被拒时不能改动文档");
    }

    #[test]
    fn next_field_moves_the_caret_with_declared_capabilities() {
        let host = host_with(&declared_capabilities());
        host.open_editor("/p/a.csv", "csv", "a,b\nc,d\n", &[(0, 0, 0, 0)]);
        RainbowCsv
            .run_command("next-field", &host.invocation(), None)
            .expect("next-field");
        assert_eq!(
            host.revealed_ranges(),
            vec![Range::new(Position::new(0, 2), Position::new(0, 3))]
        );
    }

    #[test]
    fn show_delimiter_reports_through_declared_capabilities() {
        let host = host_with(&declared_capabilities());
        host.open_editor("/p/a.csv", "csv", "a,b,c\n1,2,3\n", &[(0, 0, 0, 0)]);
        let body = RainbowCsv
            .run_command("show-delimiter", &host.invocation(), None)
            .expect("show-delimiter")
            .expect("返回值");
        assert_eq!(body, "Delimiter: comma (,) — 2 rows, 3 columns");
    }

    #[test]
    fn public_highlights_entry_colours_every_column() {
        // 走公开的 SyntaxProvider 入口，而不是私有 tokens_for。
        let _host = MockHost::install();
        let text = "id,name,role\n1,ann,ops\n2,bob,qa\n";
        let toks = SyntaxProvider::highlights(&mut RainbowCsv, "csv", text);
        assert_eq!(toks.len(), 9);
        let first_row: Vec<_> = toks[..3].iter().map(|t| t.kind).collect();
        let second_row: Vec<_> = toks[3..6].iter().map(|t| t.kind).collect();
        assert_eq!(first_row, second_row, "同一列上下同色");
        assert_eq!(toks[0].kind, toks[6].kind);
        assert_ne!(toks[0].kind, toks[1].kind, "相邻列不同色");
    }

    #[test]
    fn crlf_file_is_coloured_and_aligned() {
        let host = host_with(&declared_capabilities());
        host.open_editor("/p/win.csv", "csv", "a,bb\r\nccc,d\r\n", &[(0, 0, 0, 0)]);
        RainbowCsv
            .run_command("align-columns", &host.invocation(), None)
            .expect("align");
        let text = host.editor_text();
        assert!(text.contains("a  ,bb"), "对齐结果: {text:?}");
        let _ = &text;
    }

    #[test]
    fn quoted_second_column_keeps_comma_and_align_leaves_its_content_alone() {
        // 评审给的复现：第 2 列整体是引号字段，里面的分号不能改变检测结果
        let host = host_with(&declared_capabilities());
        let text = "id,notes\n1,\"a;b;c\"\n2,\"long;b;c\"\n";
        host.open_editor("/p/quoted.csv", "csv", text, &[(0, 0, 0, 0)]);

        let body = RainbowCsv
            .run_command("show-delimiter", &host.invocation(), None)
            .expect("show-delimiter")
            .expect("返回值");
        assert_eq!(body, "Delimiter: comma (,) — 3 rows, 2 columns");

        RainbowCsv
            .run_command("align-columns", &host.invocation(), None)
            .expect("align");
        assert_eq!(
            host.editor_text(),
            "id,notes\n1 ,\"a;b;c\"\n2 ,\"long;b;c\"\n",
            "对齐只能在引号外补空格"
        );
    }

    #[test]
    fn headerless_semicolon_file_is_not_mistaken_for_commas() {
        // 没有表头时两种切法都「整齐」，comma 会切开引号字段，必须排除
        let text = "1;\"a, b, c\"\n2;\"long, b, c\"\n";
        assert_eq!(csv::detect_delimiter(text), b';');
    }

    #[test]
    fn headerless_semicolon_file_survives_trim_cells() {
        let host = host_with(&declared_capabilities());
        let text = "1;\"a, b, c\"\n2;\"long, b, c\"\n";
        host.open_editor("/p/h.csv", "csv", text, &[(0, 0, 0, 0)]);

        let body = RainbowCsv
            .run_command("show-delimiter", &host.invocation(), None)
            .expect("show-delimiter")
            .expect("返回值");
        assert_eq!(body, "Delimiter: semicolon (;) — 2 rows, 2 columns");

        RainbowCsv
            .run_command("trim-cells", &host.invocation(), None)
            .expect("trim");
        assert_eq!(host.editor_text(), text, "引号内的空格不是边界空白");
    }

    #[test]
    fn trim_refuses_when_the_delimiter_splits_inside_quotes() {
        // 设置里固定了错误的分隔符：trim 也不能动，否则删的是引号内部空格
        let host = host_with(&declared_capabilities());
        host.set_setting("delimiter", "\",\"");
        let text = "1;\"a, b, c\"\n";
        host.open_editor("/p/h2.csv", "csv", text, &[(0, 0, 0, 0)]);

        RainbowCsv
            .run_command("trim-cells", &host.invocation(), None)
            .expect("trim");
        assert_eq!(host.editor_text(), text);
    }
}
