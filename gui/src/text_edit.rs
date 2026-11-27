use serde_json::Value;

#[derive(Clone, Debug)]
pub struct TextEditError {
    pub code: &'static str,
    pub message: String,
    pub matches: i32,
}

impl TextEditError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            matches: -1,
        }
    }

    pub fn with_matches(code: &'static str, message: impl Into<String>, matches: i32) -> Self {
        Self {
            code,
            message: message.into(),
            matches,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TextLine {
    pub begin: usize,
    pub text_end: usize,
    pub end: usize,
}

pub fn text_lines(text: &str) -> Vec<TextLine> {
    let mut rows = Vec::new();
    let bytes = text.as_bytes();
    let mut pos = if bytes.starts_with(b"\xEF\xBB\xBF") {
        3
    } else {
        0
    };
    while pos < bytes.len() {
        let lf = bytes[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|rel| pos + rel);
        let next = lf.map(|i| i + 1).unwrap_or(bytes.len());
        let mut stop = lf.unwrap_or(bytes.len());
        if stop > pos && bytes[stop - 1] == b'\r' {
            stop -= 1;
        }
        rows.push(TextLine {
            begin: pos,
            text_end: stop,
            end: next,
        });
        pos = next;
    }
    rows
}

pub fn text_eol(text: &str) -> &'static str {
    let bytes = text.as_bytes();
    if let Some(at) = bytes.iter().position(|&b| b == b'\n') {
        if at > 0 && bytes[at - 1] == b'\r' {
            return "\r\n";
        }
    }
    "\n"
}

pub fn normalize_eol(text: &str, eol: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\r' && i + 1 < chars.len() && chars[i + 1] == '\n' {
            i += 1;
            continue;
        }
        if chars[i] == '\n' {
            out.push_str(eol);
        } else {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

pub fn apply_text_edit(before: &str, args: &Value) -> Result<String, TextEditError> {
    let operation = args
        .get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| TextEditError::new("EDIT_OPERATION", "operationが必要です"))?;
    let eol = text_eol(before);
    let mut after = before.to_string();

    if operation == "replace" {
        let old_raw = args
            .get("oldText")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("EMPTY_ANCHOR", "oldTextが必要です"))?;
        let new_raw = args
            .get("newText")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("EDIT_OPERATION", "newTextが必要です"))?;
        let old = normalize_eol(old_raw, eol);
        let next = normalize_eol(new_raw, eol);
        if old.is_empty() {
            return Err(TextEditError::new(
                "EMPTY_ANCHOR",
                "置換元oldTextは空にできません。挿入にはlocal.insertを使ってください",
            ));
        }
        let mut at = 0usize;
        let mut found = 0usize;
        let mut count = 0i32;
        while let Some(rel) = before[at..].find(&old) {
            found = at + rel;
            count += 1;
            at = found + old.len().max(1);
        }
        if count != 1 {
            let code = if count > 0 {
                "AMBIGUOUS_MATCH"
            } else {
                "TEXT_NOT_FOUND"
            };
            return Err(TextEditError::with_matches(
                code,
                format!(
                    "置換元が{}箇所あります。書き込みは行っていません。前後を含む一意なoldTextを指定してください",
                    count
                ),
                count,
            ));
        }
        after.replace_range(found..found + old.len(), &next);
    } else {
        let lines = text_lines(before);
        let count = lines.len();
        let bom_len = if before.as_bytes().starts_with(b"\xEF\xBB\xBF") {
            3usize
        } else {
            0usize
        };
        if operation == "insert" {
            let line = args
                .get("line")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| TextEditError::new("LINE_RANGE", "lineが必要です"))?;
            let position = args
                .get("position")
                .and_then(|v| v.as_str())
                .ok_or_else(|| TextEditError::new("LINE_RANGE", "positionが必要です"))?;
            if line < 1
                || (count > 0 && (line as usize) > count)
                || (count == 0 && line != 1)
                || (position != "before" && position != "after")
            {
                return Err(TextEditError::new(
                    "LINE_RANGE",
                    "挿入先は現在の1始まりの行番号とbefore/afterで指定してください",
                ));
            }
            let text_raw = args
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| TextEditError::new("EMPTY_INSERT", "挿入するtextが空です"))?;
            let mut text = normalize_eol(text_raw, eol);
            if text.is_empty() {
                return Err(TextEditError::new(
                    "EMPTY_INSERT",
                    "挿入するtextが空です",
                ));
            }
            let at = if count > 0 {
                let idx = (line as usize) - 1;
                if position == "before" {
                    lines[idx].begin
                } else {
                    lines[idx].end
                }
            } else {
                bom_len
            };
            if at > bom_len && before.as_bytes()[at - 1] != b'\n' {
                text = format!("{}{}", eol, text);
            }
            if (at < before.len() || (!before.is_empty() && before.ends_with('\n')))
                && !text.ends_with('\n')
            {
                text.push_str(eol);
            }
            after.insert_str(at, &text);
        } else if operation == "delete_lines" || operation == "replace_lines" {
            let first = args
                .get("startLine")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| TextEditError::new("LINE_RANGE", "startLineが必要です"))?;
            let remove = args
                .get("deleteCount")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| TextEditError::new("LINE_RANGE", "deleteCountが必要です"))?;
            if first < 1
                || remove < 1
                || (first as usize) > count
                || (remove as usize) > count - (first as usize) + 1
            {
                return Err(TextEditError::new(
                    "LINE_RANGE",
                    format!(
                        "startLineは1始まり、deleteCountは削除する行数です。現在の総行数は{}です。範囲外なので変更していません",
                        count
                    ),
                ));
            }
            let first_u = first as usize;
            let remove_u = remove as usize;
            let begin = lines[first_u - 1].begin;
            let end = lines[first_u + remove_u - 2].end;
            let mut text = if operation == "replace_lines" {
                let raw = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| TextEditError::new("EDIT_OPERATION", "textが必要です"))?;
                normalize_eol(raw, eol)
            } else {
                String::new()
            };
            if !text.is_empty()
                && (end < before.len() || (!before.is_empty() && before.ends_with('\n')))
                && !text.ends_with('\n')
            {
                text.push_str(eol);
            }
            after.replace_range(begin..end, &text);
        } else {
            return Err(TextEditError::new("EDIT_OPERATION", "不明な編集操作です"));
        }
    }

    if after.len() > 65536 {
        return Err(TextEditError::new(
            "FILE_TOO_LARGE",
            "編集後のファイルが64KiBを超えます。変更していません",
        ));
    }
    if after.as_bytes().contains(&0) {
        return Err(TextEditError::new(
            "BINARY_FILE",
            "バイナリの編集はできません",
        ));
    }
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_edit_operations_preserve_eol_and_validate() {
        let src = "line1\r\nline2\r\nline3\r\n";
        let replaced = apply_text_edit(
            src,
            &json!({"operation": "replace", "oldText": "line2", "newText": "LINE_TWO"}),
        )
        .unwrap();
        assert_eq!(replaced, "line1\r\nLINE_TWO\r\nline3\r\n");

        let inserted = apply_text_edit(
            src,
            &json!({"operation": "insert", "line": 2, "position": "before", "text": "mid"}),
        )
        .unwrap();
        assert_eq!(inserted, "line1\r\nmid\r\nline2\r\nline3\r\n");

        let deleted = apply_text_edit(
            src,
            &json!({"operation": "delete_lines", "startLine": 2, "deleteCount": 1}),
        )
        .unwrap();
        assert_eq!(deleted, "line1\r\nline3\r\n");

        let err = apply_text_edit(
            "dup\ndup\n",
            &json!({"operation": "replace", "oldText": "dup", "newText": "x"}),
        )
        .unwrap_err();
        assert_eq!(err.code, "AMBIGUOUS_MATCH");
        assert_eq!(err.matches, 2);
    }
}
