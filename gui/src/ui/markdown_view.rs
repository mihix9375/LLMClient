use crate::ui::selectable::{build_line_highlights, LineHighlights, SelectableText};
use crate::ui::selection_scope::SelectionScope;
use crate::ui::theme::{
    self, MONO_FONT, MONO_FONT_BOLD, UI_FONT, UI_FONT_BOLD, UI_FONT_ITALIC, UiPalette,
};
use iced::advanced::text::highlighter;
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, rule, scrollable, space, text};
use iced::{Alignment, Color, Element, Length, Padding, Shadow};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub struct MdSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub bullet: String,
    pub spans: Vec<MdSpan>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MdBlock {
    Heading { level: u8, spans: Vec<MdSpan> },
    Paragraph(Vec<MdSpan>),
    CodeCard { lang: String, code: String },
    Blockquote(Vec<Vec<MdSpan>>),
    List { items: Vec<ListItem> },
    Table {
        headers: Vec<Vec<MdSpan>>,
        rows: Vec<Vec<Vec<MdSpan>>>,
    },
    Hr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokKind {
    Normal,
    Keyword,
    StringLit,
    Number,
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Ctx,
    Add,
    Del,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiffRow {
    pub kind: DiffKind,
    pub ln_a: Option<usize>,
    pub ln_b: Option<usize>,
    pub text: String,
}

fn push_plain(out: &mut Vec<MdSpan>, s: &str, bold: bool, italic: bool, strike: bool) {
    if !s.is_empty() {
        out.push(MdSpan {
            text: s.to_string(),
            bold,
            italic,
            strike,
            code: false,
            link: None,
        });
    }
}

pub fn parse_inline(raw: &str) -> Vec<MdSpan> {
    let mut out = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    let mut buf = String::new();

    let flush_buf = |b: &mut String, dst: &mut Vec<MdSpan>| {
        if !b.is_empty() {
            push_plain(dst, b, false, false, false);
            b.clear();
        }
    };

    while i < raw.len() {
        let rest = &raw[i..];
        if bytes[i] == b'`' {
            if let Some(end) = rest[1..].find('`') {
                flush_buf(&mut buf, &mut out);
                out.push(MdSpan {
                    text: rest[1..1 + end].to_string(),
                    bold: false,
                    italic: false,
                    strike: false,
                    code: true,
                    link: None,
                });
                i += 2 + end;
                continue;
            }
        }
        if bytes[i] == b'[' {
            if let Some(mid) = rest[1..].find("](") {
                let label = &rest[1..1 + mid];
                let after = &rest[1 + mid + 2..];
                if let Some(close) = after.find(')') {
                    let url = &after[..close];
                    if (url.starts_with("http://")
                        || url.starts_with("https://")
                        || url.starts_with("attachment://"))
                        && !url.contains(char::is_whitespace)
                    {
                        flush_buf(&mut buf, &mut out);
                        out.push(MdSpan {
                            text: label.to_string(),
                            bold: false,
                            italic: false,
                            strike: false,
                            code: false,
                            link: Some(url.to_string()),
                        });
                        i += 1 + mid + 2 + close + 1;
                        continue;
                    }
                }
            }
        }
        if rest.starts_with("**") {
            if let Some(end) = rest[2..].find("**") {
                if end > 0 {
                    flush_buf(&mut buf, &mut out);
                    push_plain(&mut out, &rest[2..2 + end], true, false, false);
                    i += 4 + end;
                    continue;
                }
            }
        }
        if rest.starts_with("__") {
            if let Some(end) = rest[2..].find("__") {
                if end > 0 {
                    flush_buf(&mut buf, &mut out);
                    push_plain(&mut out, &rest[2..2 + end], true, false, false);
                    i += 4 + end;
                    continue;
                }
            }
        }
        if rest.starts_with("~~") {
            if let Some(end) = rest[2..].find("~~") {
                if end > 0 {
                    flush_buf(&mut buf, &mut out);
                    push_plain(&mut out, &rest[2..2 + end], false, false, true);
                    i += 4 + end;
                    continue;
                }
            }
        }
        if bytes[i] == b'*' && !rest.starts_with("**") {
            if let Some(end) = rest[1..].find('*') {
                if end > 0 && !rest[1..1 + end].contains('\n') {
                    flush_buf(&mut buf, &mut out);
                    push_plain(&mut out, &rest[1..1 + end], false, true, false);
                    i += 2 + end;
                    continue;
                }
            }
        }
        let ch = rest.chars().next().unwrap();
        buf.push(ch);
        i += ch.len_utf8();
    }
    flush_buf(&mut buf, &mut out);
    if out.is_empty() {
        out.push(MdSpan {
            text: String::new(),
            bold: false,
            italic: false,
            strike: false,
            code: false,
            link: None,
        });
    }
    out
}

fn is_hr(t: &str) -> bool {
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

fn is_table_sep(t: &str) -> bool {
    let s = t.trim();
    s.contains('|')
        && s.chars()
            .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
}

fn parse_table_row(line: &str) -> Vec<Vec<MdSpan>> {
    let mut s = line.trim();
    if let Some(rest) = s.strip_prefix('|') {
        s = rest;
    }
    if let Some(rest) = s.strip_suffix('|') {
        s = rest;
    }
    s.split('|').map(|c| parse_inline(c.trim())).collect()
}

fn ordered_prefix(t: &str) -> Option<(String, &str)> {
    let dot = t.find(". ")?;
    let num = &t[..dot];
    if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
        Some((format!("{}.", num), &t[dot + 2..]))
    } else {
        None
    }
}

fn unordered_prefix(t: &str) -> Option<&str> {
    t.strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
}

fn is_valid_fence_lang(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 24
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'#' | b'.' | b'-'))
}

pub fn parse_markdown(raw: &str) -> Vec<MdBlock> {
    let normalized = raw.replace("\r\n", "\n");
    let mut lines: Vec<&str> = normalized.split('\n').collect();
    let mut blocks = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        let ls = lines[i].trim_start();
        let fence_len = ls.bytes().take_while(|&b| b == b'`').count();
        if fence_len >= 3 {
            let after_open = &ls[fence_len..];
            let after_trimmed_end = after_open.trim_end();
            let end_ticks = after_trimmed_end
                .bytes()
                .rev()
                .take_while(|&b| b == b'`')
                .count();
            if end_ticks >= fence_len {
                let inner = &after_trimmed_end[..after_trimmed_end.len() - end_ticks];
                blocks.push(MdBlock::CodeCard {
                    lang: "code".to_string(),
                    code: inner.to_string(),
                });
                i += 1;
                continue;
            }
            let info = after_open.trim();
            let mut code_lines: Vec<&str> = Vec::new();
            let lang = if info.is_empty() {
                "code".to_string()
            } else if is_valid_fence_lang(info) {
                info.to_string()
            } else {
                code_lines.push(after_trimmed_end);
                "code".to_string()
            };
            i += 1;
            while i < lines.len() {
                let line = lines[i];
                let line_ls = line.trim_start();
                let lead_ticks = line_ls.bytes().take_while(|&b| b == b'`').count();
                if lead_ticks >= fence_len {
                    let remainder = line_ls[lead_ticks..].trim_start();
                    if remainder.is_empty() {
                        i += 1;
                    } else {
                        lines[i] = remainder;
                    }
                    break;
                }
                let line_te = line.trim_end();
                let trail_ticks = line_te.bytes().rev().take_while(|&b| b == b'`').count();
                if trail_ticks >= fence_len {
                    let prefix = &line_te[..line_te.len() - trail_ticks];
                    if !prefix.is_empty() {
                        code_lines.push(prefix);
                    }
                    i += 1;
                    break;
                }
                code_lines.push(line);
                i += 1;
            }
            blocks.push(MdBlock::CodeCard {
                lang,
                code: code_lines.join("\n"),
            });
            continue;
        }
        if let Some((hashes, rest)) = trimmed.split_once(' ') {
            let lvl = hashes.len();
            if (1..=4).contains(&lvl) && hashes.bytes().all(|b| b == b'#') {
                blocks.push(MdBlock::Heading {
                    level: lvl as u8,
                    spans: parse_inline(rest.trim()),
                });
                i += 1;
                continue;
            }
        }
        if is_hr(trimmed) {
            blocks.push(MdBlock::Hr);
            i += 1;
            continue;
        }
        if trimmed.starts_with('>') {
            let mut quote_lines = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('>') {
                let l = lines[i].trim();
                let body = l.strip_prefix("> ").or_else(|| l.strip_prefix('>')).unwrap_or("");
                quote_lines.push(parse_inline(body));
                i += 1;
            }
            blocks.push(MdBlock::Blockquote(quote_lines));
            continue;
        }
        if trimmed.contains('|') && i + 1 < lines.len() && is_table_sep(lines[i + 1]) {
            let headers = parse_table_row(trimmed);
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim().contains('|') {
                rows.push(parse_table_row(lines[i]));
                i += 1;
            }
            blocks.push(MdBlock::Table { headers, rows });
            continue;
        }
        if unordered_prefix(trimmed).is_some() || ordered_prefix(trimmed).is_some() {
            let ordered = ordered_prefix(trimmed).is_some();
            let mut items = Vec::new();
            while i < lines.len() {
                let lt = lines[i].trim();
                if ordered {
                    if let Some((num, body)) = ordered_prefix(lt) {
                        items.push(ListItem {
                            bullet: num,
                            spans: parse_inline(body),
                        });
                        i += 1;
                        continue;
                    }
                } else if let Some(body) = unordered_prefix(lt) {
                    let (bullet, content) = if let Some(rest) = body.strip_prefix("[ ] ") {
                        ("☐".to_string(), rest)
                    } else if let Some(rest) = body
                        .strip_prefix("[x] ")
                        .or_else(|| body.strip_prefix("[X] "))
                    {
                        ("☑".to_string(), rest)
                    } else {
                        ("•".to_string(), body)
                    };
                    items.push(ListItem {
                        bullet,
                        spans: parse_inline(content),
                    });
                    i += 1;
                    continue;
                }
                break;
            }
            blocks.push(MdBlock::List { items });
            continue;
        }

        let mut para = Vec::new();
        while i < lines.len() {
            let lt = lines[i].trim();
            if lt.is_empty()
                || lt.starts_with("```")
                || lt.starts_with('>')
                || is_hr(lt)
                || unordered_prefix(lt).is_some()
                || ordered_prefix(lt).is_some()
                || lt
                    .split_once(' ')
                    .is_some_and(|(h, _)| (1..=4).contains(&h.len()) && h.bytes().all(|b| b == b'#'))
            {
                break;
            }
            if !para.is_empty() {
                para.push(MdSpan {
                    text: "\n".to_string(),
                    bold: false,
                    italic: false,
                    strike: false,
                    code: false,
                    link: None,
                });
            }
            para.extend(parse_inline(lines[i]));
            i += 1;
        }
        if !para.is_empty() {
            blocks.push(MdBlock::Paragraph(para));
        }
    }
    blocks
}

fn is_keyword(w: &str) -> bool {
    matches!(
        w,
        "fn" | "let"
            | "mut"
            | "pub"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "match"
            | "if"
            | "else"
            | "for"
            | "while"
            | "loop"
            | "return"
            | "use"
            | "mod"
            | "crate"
            | "async"
            | "await"
            | "const"
            | "static"
            | "unsafe"
            | "where"
            | "type"
            | "true"
            | "false"
            | "null"
            | "None"
            | "Some"
            | "Ok"
            | "Err"
            | "function"
            | "var"
            | "class"
            | "import"
            | "export"
            | "from"
            | "def"
            | "try"
            | "catch"
            | "throw"
            | "finally"
            | "switch"
            | "case"
            | "break"
            | "continue"
            | "int"
            | "void"
            | "bool"
            | "auto"
    )
}

pub fn highlight_code(raw: &str, lang: &str) -> Vec<Vec<(TokKind, String)>> {
    let l = lang.to_ascii_lowercase();
    let hash_cmt = matches!(
        l.as_str(),
        "py" | "python" | "bash" | "sh" | "ps1" | "powershell" | "toml" | "yaml" | "yml"
    );
    let no_hi = l.is_empty() || l == "text" || l == "txt" || l == "stdout" || l == "stderr";

    raw.lines()
        .map(|line| {
            if no_hi {
                return vec![(TokKind::Normal, line.to_string())];
            }
            let mut toks = Vec::new();
            let mut chars = line.char_indices().peekable();
            while let Some((idx, ch)) = chars.peek().copied() {
                let rest = &line[idx..];
                if rest.starts_with("//") || (hash_cmt && ch == '#') {
                    toks.push((TokKind::Comment, rest.to_string()));
                    break;
                }
                if ch == '"' || ch == '\'' {
                    let quote = ch;
                    let mut end = idx + ch.len_utf8();
                    chars.next();
                    let mut escaped = false;
                    while let Some((j, c2)) = chars.next() {
                        end = j + c2.len_utf8();
                        if escaped {
                            escaped = false;
                        } else if c2 == '\\' {
                            escaped = true;
                        } else if c2 == quote {
                            break;
                        }
                    }
                    toks.push((TokKind::StringLit, line[idx..end].to_string()));
                    continue;
                }
                if ch.is_ascii_digit() {
                    let mut end = idx + 1;
                    chars.next();
                    while let Some((j, c2)) = chars.peek().copied() {
                        if c2.is_ascii_digit() || c2 == '.' {
                            end = j + 1;
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    toks.push((TokKind::Number, line[idx..end].to_string()));
                    continue;
                }
                if ch.is_ascii_alphabetic() || ch == '_' {
                    let mut end = idx + ch.len_utf8();
                    chars.next();
                    while let Some((j, c2)) = chars.peek().copied() {
                        if c2.is_ascii_alphanumeric() || c2 == '_' {
                            end = j + c2.len_utf8();
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    let word = &line[idx..end];
                    let kind = if is_keyword(word) {
                        TokKind::Keyword
                    } else {
                        TokKind::Normal
                    };
                    toks.push((kind, word.to_string()));
                    continue;
                }
                toks.push((TokKind::Normal, ch.to_string()));
                chars.next();
            }
            if toks.is_empty() {
                toks.push((TokKind::Normal, String::new()));
            }
            toks
        })
        .collect()
}

pub fn compute_line_diff(before: &str, after: &str) -> Option<Vec<DiffRow>> {
    let a: Vec<&str> = before.split('\n').collect();
    let b: Vec<&str> = after.split('\n').collect();
    if a.len() > 260 || b.len() > 260 {
        return None;
    }
    let mut prefix = 0usize;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < a.len() - prefix
        && suffix < b.len() - prefix
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mut rows = Vec::new();
    let ctx_start = prefix.saturating_sub(2);
    for (idx, line) in a.iter().enumerate().take(prefix).skip(ctx_start) {
        rows.push(DiffRow {
            kind: DiffKind::Ctx,
            ln_a: Some(idx + 1),
            ln_b: Some(idx + 1),
            text: (*line).to_string(),
        });
    }
    for (idx, line) in a.iter().enumerate().take(a.len() - suffix).skip(prefix) {
        rows.push(DiffRow {
            kind: DiffKind::Del,
            ln_a: Some(idx + 1),
            ln_b: None,
            text: (*line).to_string(),
        });
    }
    for (idx, line) in b.iter().enumerate().take(b.len() - suffix).skip(prefix) {
        rows.push(DiffRow {
            kind: DiffKind::Add,
            ln_a: None,
            ln_b: Some(idx + 1),
            text: (*line).to_string(),
        });
    }
    let ctx_end = a.len().min(a.len() - suffix + 2);
    for (idx, line) in a.iter().enumerate().take(ctx_end).skip(a.len() - suffix) {
        let b_idx = b.len() - (a.len() - idx);
        rows.push(DiffRow {
            kind: DiffKind::Ctx,
            ln_a: Some(idx + 1),
            ln_b: Some(b_idx + 1),
            text: (*line).to_string(),
        });
    }
    Some(rows)
}

fn spans_to_selectable<'a, M: Clone + 'static>(
    spans: &[MdSpan],
    size: f32,
    default_color: Color,
    pal: UiPalette,
    force_bold: bool,
    scope: SelectionScope,
    on_link: impl Fn(String) -> M + 'static,
    on_sel_copy: impl Fn(String) -> M + 'static,
) -> Element<'a, M> {
    let mut full = String::new();
    let mut raw_hl: Vec<(Range<usize>, highlighter::Format<iced::Font>)> = Vec::new();
    let mut links: Vec<(Range<usize>, String)> = Vec::new();
    for s in spans {
        let start = full.len();
        full.push_str(&s.text);
        let end = full.len();
        if start == end {
            continue;
        }
        if s.code {
            raw_hl.push((
                start..end,
                highlighter::Format { color: Some(pal.accent), font: Some(MONO_FONT) },
            ));
        } else if let Some(ref url) = s.link {
            raw_hl.push((
                start..end,
                highlighter::Format { color: Some(pal.accent), font: None },
            ));
            links.push((start..end, url.clone()));
        } else if force_bold || s.bold {
            raw_hl.push((
                start..end,
                highlighter::Format { color: Some(default_color), font: Some(UI_FONT_BOLD) },
            ));
        } else if s.italic {
            raw_hl.push((
                start..end,
                highlighter::Format { color: Some(default_color), font: Some(UI_FONT_ITALIC) },
            ));
        }
    }
    let hl = build_line_highlights(&full, &raw_hl);
    SelectableText::new(full)
        .highlights(hl)
        .links(links)
        .size(size)
        .font(if force_bold { UI_FONT_BOLD } else { UI_FONT })
        .color(default_color)
        .selection_color(pal.accent.scale_alpha(0.28))
        .width(Length::Fill)
        .wrapping(Wrapping::WordOrGlyph)
        .scope(scope)
        .on_link(on_link)
        .on_copy(on_sel_copy)
        .into()
}

pub fn render_code_card<'a, M: Clone + 'static>(
    lang: &str,
    code: &str,
    copy_key: String,
    pal: UiPalette,
    copied_key: Option<&str>,
    scope: SelectionScope,
    on_copy: impl Fn(String, String) -> M + 'a,
    on_sel_copy: impl Fn(String) -> M + 'static,
) -> Element<'a, M> {
    let is_copied = copied_key == Some(copy_key.as_str());
    let head = container(
        row![
            text(lang.to_string()).size(11.5).color(pal.ink_muted),
            space::horizontal(),
            button(
                text(if is_copied { "コピー済 ✓" } else { "コピー" }).size(11.5)
            )
            .padding([3, 9])
            .style(theme::btn_ghost(pal))
            .on_press(on_copy(copy_key, code.to_string())),
        ]
        .align_y(Alignment::Center),
    )
    .padding([5, 12])
    .width(Length::Fill)
    .style(theme::box_style(
        pal.bg_surface_2,
        pal.border_subtle,
        0.0,
        1.0,
        Some(pal.ink_muted),
        Shadow::default(),
    ));

    let normalized = code.replace("\r\n", "\n");
    let lines = highlight_code(&normalized, lang);
    let hl: LineHighlights = lines
        .into_iter()
        .map(|toks| {
            let mut off = 0usize;
            let mut row = Vec::new();
            for (k, txt) in toks {
                let next = off + txt.len();
                if k != TokKind::Normal && next > off {
                    let (c, f) = match k {
                        TokKind::Keyword => (pal.tok_kw, MONO_FONT_BOLD),
                        TokKind::StringLit => (pal.tok_str, MONO_FONT),
                        TokKind::Number => (pal.tok_num, MONO_FONT),
                        TokKind::Comment => (pal.tok_cmt, MONO_FONT),
                        TokKind::Normal => (pal.ink, MONO_FONT),
                    };
                    row.push((off..next, highlighter::Format { color: Some(c), font: Some(f) }));
                }
                off = next;
            }
            row
        })
        .collect();

    let code_view = SelectableText::new(normalized)
        .highlights(hl)
        .font(MONO_FONT)
        .size(12.5)
        .color(pal.ink)
        .selection_color(pal.accent.scale_alpha(0.28))
        .width(Length::Shrink)
        .wrapping(Wrapping::None)
        .scope(scope)
        .nested_scroll(true)
        .on_copy(on_sel_copy);

    let code_scroll = scrollable(
        container(code_view)
            .padding([10, 14])
            .width(Length::Shrink),
    )
    .horizontal()
    .width(Length::Fill)
    .style(theme::scrollable_style(pal));

    container(column![head, code_scroll])
        .width(Length::Fill)
        .style(theme::box_style(
            pal.bg_code,
            pal.border_subtle,
            12.0,
            1.0,
            Some(pal.ink),
            Shadow::default(),
        ))
        .clip(true)
        .into()
}

pub fn render_markdown<'a, M: Clone + 'static>(
    blocks: &'a [MdBlock],
    pal: UiPalette,
    id_prefix: &str,
    copied_key: Option<&str>,
    scope: SelectionScope,
    on_copy: impl Fn(String, String) -> M + Clone + 'static,
    on_link: impl Fn(String) -> M + Clone + 'static,
    on_sel_copy: impl Fn(String) -> M + Clone + 'static,
) -> Element<'a, M> {
    if blocks.is_empty() {
        return space().into();
    }
    let mut children: Vec<Element<'a, M>> = Vec::with_capacity(blocks.len());
    for (idx, b) in blocks.iter().enumerate() {
        let el: Element<'a, M> = match b {
            MdBlock::Heading { level, spans } => {
                let sz = match level {
                    1 => 19.0,
                    2 => 16.5,
                    3 => 15.0,
                    _ => 14.0,
                };
                container(spans_to_selectable(
                    spans,
                    sz,
                    pal.ink,
                    pal,
                    true,
                    scope.clone(),
                    on_link.clone(),
                    on_sel_copy.clone(),
                ))
                .padding(Padding {
                    top: 6.0,
                    bottom: 2.0,
                    left: 0.0,
                    right: 0.0,
                })
                .into()
            }
            MdBlock::Paragraph(spans) => spans_to_selectable(
                spans,
                14.0,
                pal.ink,
                pal,
                false,
                scope.clone(),
                on_link.clone(),
                on_sel_copy.clone(),
            ),
            MdBlock::CodeCard { lang, code } => render_code_card(
                lang,
                code,
                format!("{}:code:{}", id_prefix, idx),
                pal,
                copied_key,
                scope.clone(),
                on_copy.clone(),
                on_sel_copy.clone(),
            ),
            MdBlock::Blockquote(lines) => {
                let rows = lines.iter().map(|sp| {
                    spans_to_selectable(
                        sp,
                        13.5,
                        pal.ink_secondary,
                        pal,
                        false,
                        scope.clone(),
                        on_link.clone(),
                        on_sel_copy.clone(),
                    )
                });
                container(column(rows).spacing(4))
                    .padding([8, 12])
                    .width(Length::Fill)
                    .style(theme::blockquote_style(pal))
                    .into()
            }
            MdBlock::List { items } => {
                let rows = items.iter().map(|it| {
                    row![
                        text(it.bullet.clone())
                            .size(13.5)
                            .color(pal.accent)
                            .width(Length::Fixed(20.0)),
                        spans_to_selectable(
                            &it.spans,
                            14.0,
                            pal.ink,
                            pal,
                            false,
                            scope.clone(),
                            on_link.clone(),
                            on_sel_copy.clone(),
                        ),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Start)
                    .into()
                });
                column(rows).spacing(4).padding([2, 4]).into()
            }
            MdBlock::Table { headers, rows } => {
                let head_row = row(headers.iter().map(|cell| {
                    container(spans_to_selectable(
                        cell,
                        12.5,
                        pal.ink,
                        pal,
                        true,
                        scope.clone(),
                        on_link.clone(),
                        on_sel_copy.clone(),
                    ))
                    .width(Length::FillPortion(1))
                    .padding([6, 8])
                    .style(theme::surface2_style(pal, 0.0))
                    .into()
                }));
                let body_rows = rows.iter().map(|r| {
                    Element::from(row(r.iter().map(|cell| {
                        container(spans_to_selectable(
                            cell,
                            12.5,
                            pal.ink_secondary,
                            pal,
                            false,
                            scope.clone(),
                            on_link.clone(),
                            on_sel_copy.clone(),
                        ))
                        .width(Length::FillPortion(1))
                        .padding([5, 8])
                        .style(theme::box_style(
                            pal.bg_surface,
                            pal.border_subtle,
                            0.0,
                            0.5,
                            Some(pal.ink),
                            Shadow::default(),
                        ))
                        .into()
                    })))
                });
                container(column![head_row].push(column(body_rows)))
                    .width(Length::Fill)
                    .style(theme::box_style(
                        pal.bg_surface,
                        pal.border_subtle,
                        8.0,
                        1.0,
                        Some(pal.ink),
                        Shadow::default(),
                    ))
                    .clip(true)
                    .into()
            }
            MdBlock::Hr => rule::horizontal(1).style(theme::rule_subtle(pal)).into(),
        };
        children.push(el);
    }
    column(children).spacing(8).width(Length::Fill).into()
}

pub fn render_diff_card<'a, M: 'a>(before: &str, after: &str, pal: UiPalette) -> Element<'a, M> {
    let Some(rows) = compute_line_diff(before, after) else {
        return text(format!(
            "差分が大きいため行比較を省略しました ({} bytes)",
            after.len()
        ))
        .size(12.0)
        .color(pal.ink_muted)
        .into();
    };
    if rows.is_empty() {
        return text("変更差分はありません")
            .size(12.0)
            .color(pal.ink_muted)
            .into();
    }
    let list = column(rows.into_iter().take(180).map(|r| {
        let (bg, fg, mark) = match r.kind {
            DiffKind::Add => (pal.diff_add_bg, pal.green, "+"),
            DiffKind::Del => (pal.diff_del_bg, pal.red, "-"),
            DiffKind::Ctx => (Color::TRANSPARENT, pal.ink_secondary, " "),
        };
        let la = r.ln_a.map(|n| n.to_string()).unwrap_or_default();
        let lb = r.ln_b.map(|n| n.to_string()).unwrap_or_default();
        container(
            row![
                text(la).size(11.5).font(MONO_FONT).color(pal.ink_muted).width(Length::Fixed(32.0)),
                text(lb).size(11.5).font(MONO_FONT).color(pal.ink_muted).width(Length::Fixed(32.0)),
                text(format!("{} {}", mark, r.text)).size(12.0).font(MONO_FONT).color(fg),
            ]
            .spacing(6),
        )
        .padding([2, 8])
        .width(Length::Fill)
        .style(move |_| iced::widget::container::Style {
            background: Some(iced::Background::Color(bg)),
            ..iced::widget::container::Style::default()
        })
        .into()
    }))
    .spacing(1);

    container(scrollable(list).horizontal())
        .width(Length::Fill)
        .style(theme::box_style(
            pal.bg_code,
            pal.border_subtle,
            8.0,
            1.0,
            Some(pal.ink),
            Shadow::default(),
        ))
        .clip(true)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markdown_headings_code_tables_and_lists() {
        let md = "# Title\n\nText with `code` and **bold** and [link](https://example.com).\n\n```rust\nfn main() { let x = 1; }\n```\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n- [x] Done\n- Next\n";
        let blocks = parse_markdown(md);
        assert!(matches!(blocks[0], MdBlock::Heading { level: 1, .. }));
        assert!(matches!(blocks[1], MdBlock::Paragraph(_)));
        assert!(matches!(blocks[2], MdBlock::CodeCard { .. }));
        assert!(matches!(blocks[3], MdBlock::Table { .. }));
        assert!(matches!(blocks[4], MdBlock::List { .. }));
    }

    #[test]
    fn line_diff_detects_additions_and_deletions() {
        let rows = compute_line_diff("a\nb\nc", "a\nB\nc").unwrap();
        assert!(rows.iter().any(|r| r.kind == DiffKind::Del && r.text == "b"));
        assert!(rows.iter().any(|r| r.kind == DiffKind::Add && r.text == "B"));
    }

    #[test]
    fn fenced_code_handles_trailing_fence_and_non_language_info_string() {
        let md = concat!(
            "```n     steps  peak/n  odd#  binary (last 32 bits)\n",
            "254912509    951    3791.95  357    ...11001100011010011111111101``` \n",
            "- **末尾ゼロの分布**: 観察メモ\n",
            "### なぜこれが「証明」にならないか\n",
        );
        let blocks = parse_markdown(md);
        assert_eq!(blocks.len(), 3, "got blocks: {:?}", blocks);
        match &blocks[0] {
            MdBlock::CodeCard { lang, code } => {
                assert_eq!(lang, "code");
                assert_eq!(
                    code,
                    "n     steps  peak/n  odd#  binary (last 32 bits)\n254912509    951    3791.95  357    ...11001100011010011111111101"
                );
            }
            other => panic!("expected CodeCard, got {:?}", other),
        }
        match &blocks[1] {
            MdBlock::List { items } => {
                assert_eq!(items.len(), 1);
                assert!(items[0].spans.iter().any(|s| s.bold && s.text == "末尾ゼロの分布"));
            }
            other => panic!("expected List, got {:?}", other),
        }
        match &blocks[2] {
            MdBlock::Heading { level, spans } => {
                assert_eq!(*level, 3);
                let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
                assert_eq!(joined, "なぜこれが「証明」にならないか");
            }
            other => panic!("expected Heading, got {:?}", other),
        }
    }

    #[test]
    fn fenced_code_handles_four_backtick_nested_single_line_and_leading_close_remainder() {
        // 4-backtick outer fence safely wraps inner 3-backtick example (both leading and trailing).
        let nested = "````markdown\n```rust\nlet x = 1;```\n```\n````\n";
        let blocks = parse_markdown(nested);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            MdBlock::CodeCard { lang, code } => {
                assert_eq!(lang, "markdown");
                assert_eq!(code, "```rust\nlet x = 1;```\n```");
            }
            other => panic!("expected CodeCard, got {:?}", other),
        }

        // Single-line fenced block.
        let single = "```foo```\n\nAfter paragraph\n";
        let blocks2 = parse_markdown(single);
        assert_eq!(blocks2.len(), 2);
        assert_eq!(
            blocks2[0],
            MdBlock::CodeCard {
                lang: "code".to_string(),
                code: "foo".to_string(),
            }
        );
        assert!(matches!(blocks2[1], MdBlock::Paragraph(_)));

        // Trailing Markdown immediately after a leading closing fence is not silently dropped.
        let attached_after = "```rust\nlet a = 1;\n```### 次セクション\n";
        let blocks3 = parse_markdown(attached_after);
        assert_eq!(blocks3.len(), 2, "got blocks: {:?}", blocks3);
        assert_eq!(
            blocks3[0],
            MdBlock::CodeCard {
                lang: "rust".to_string(),
                code: "let a = 1;".to_string(),
            }
        );
        match &blocks3[1] {
            MdBlock::Heading { level, spans } => {
                assert_eq!(*level, 3);
                let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
                assert_eq!(joined, "次セクション");
            }
            other => panic!("expected Heading, got {:?}", other),
        }

        // Overlong (>24 chars) or invalid-char single token falls back to code + first line.
        let overlong = "```abcdefghijklmnopqrstuvwxyz\nline2\n```\n";
        let blocks4 = parse_markdown(overlong);
        assert_eq!(
            blocks4[0],
            MdBlock::CodeCard {
                lang: "code".to_string(),
                code: "abcdefghijklmnopqrstuvwxyz\nline2".to_string(),
            }
        );
    }
}
