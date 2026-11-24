//! CSV / TSV 纯文本表格解析模块。
//!
//! 复用 `Kind::XLSX` 与 `spreadsheet::Sheet` 跨平台渲染模型，直接接入现有表格绘制、
//! 单元格选区、TSV 剪贴板复制、全文检索与 PDF/PNG 导出能力。
//!
//! 特性：
//! - 编码自动识别：UTF-8（含/不含 BOM）、UTF-16LE/BE（带 BOM）及 GB18030/GBK 回退；
//! - 分隔符自动识别：`.tsv` 固定使用 `\t`，`.csv` 支持 Excel `sep=<char>` 指令及 `,` / `;` / `\t` 探测；
//! - 遵循 RFC 4180（双引号包裹、双引号转义 `""`、引号内换行）并兼容常见非严格格式；
//! - 复用 XLSX 同等级安全预算（行数、列数、单元格数与文本字节上限），超限时截断并附带告警。

use crate::metadata::Metadata;
use crate::model::{is_east_asian_char, Cell, CellStyle, Document, Kind, Sheet, VerticalAlignment};
use crate::spreadsheet::SheetGeometry;
use crate::{Error, Result, Scope};
use std::borrow::Cow;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// 完整解析时最多读取的原始文件字节数（16 MiB）
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// 首屏封面模式最多读取的原始文件字节数（128 KiB）
const FIRST_VIEWPORT_READ_BYTES: u64 = 128 * 1024;
/// 首屏封面模式最多解析的行数（足够填满封面缩略图，避免扫描巨型文件）
const FIRST_VIEWPORT_MAX_ROWS: usize = 64;
/// 元数据速读时用于二进制探测的字节上限（4 KiB）
const METADATA_PROBE_BYTES: u64 = 4 * 1024;

const MAX_ROWS: usize = 10_000;
const MAX_COLUMNS: usize = 256;
const MAX_CELLS: usize = 50_000;
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_CELL_UTF16_LEN: usize = 4_096;

const DEFAULT_ROW_HEIGHT: f32 = 20.0;
const LINE_HEIGHT: f32 = 15.0;
const MAX_ROW_HEIGHT: f32 = 240.0;
const DEFAULT_COLUMN_WIDTH: f32 = 72.0;
const MIN_COLUMN_WIDTH: f32 = 54.0;
const MAX_COLUMN_WIDTH: f32 = 260.0;

/// 分隔符文本表格的具体扩展名种类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DelimitedKind {
    Csv,
    Tsv,
}

/// 根据文件扩展名判断是否属于支持的分隔符表格（`.csv` / `.tsv`）
pub(crate) fn detect_path_kind(path: &Path) -> Option<DelimitedKind> {
    let ext = path.extension()?.to_str()?;
    if ext.eq_ignore_ascii_case("csv") {
        Some(DelimitedKind::Csv)
    } else if ext.eq_ignore_ascii_case("tsv") {
        Some(DelimitedKind::Tsv)
    } else {
        None
    }
}

/// 判断字节前缀是否为 ZIP 归档头（用于识别被误改名为 `.csv` 的真实 OOXML 包）
pub(crate) fn has_zip_signature(file: &mut File) -> Result<bool> {
    use std::io::{Seek, SeekFrom};
    let mut header = [0_u8; 4];
    let n = file.read(&mut header)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(
        n >= 4
            && (header == *b"PK\x03\x04" || header == *b"PK\x05\x06" || header == *b"PK\x07\x08"),
    )
}

/// 从已打开的文件句柄按指定范围解析 CSV / TSV 文档
pub(crate) fn parse_file(
    file: File,
    path: &Path,
    kind: DelimitedKind,
    scope: Scope,
) -> Result<Document> {
    let name = sheet_name_from_path(path);
    parse_reader_with_name(file, kind, &name, scope)
}

/// 轻量元数据速读：仅探测文件可读性与非二进制合法性，不扫描全表内容
pub(crate) fn metadata_file(file: File, path: &Path) -> Result<Metadata> {
    let mut probe = Vec::new();
    file.take(METADATA_PROBE_BYTES).read_to_end(&mut probe)?;
    validate_not_binary(&probe)?;
    Ok(Metadata {
        schema_version: crate::model::schema_version(),
        kind: Kind::XLSX,
        page_count: 1,
        width: 1.0,
        height: 0.0,
        sheet_names: vec![sheet_name_from_path(path)],
        warnings: Vec::new(),
    })
}

/// 从 reader 解析 CSV / TSV 内容并构建 `Kind::XLSX` 单工作表模型
pub(crate) fn parse_reader_with_name(
    reader: impl Read,
    kind: DelimitedKind,
    sheet_name: &str,
    scope: Scope,
) -> Result<Document> {
    let read_limit = match scope {
        Scope::Full => MAX_FILE_BYTES,
        Scope::FirstViewport => FIRST_VIEWPORT_READ_BYTES,
    };
    let mut raw = Vec::new();
    // 多读 1 字节用于判断是否触发了读取上限
    reader
        .take(read_limit.saturating_add(1))
        .read_to_end(&mut raw)?;
    let hit_read_limit = raw.len() as u64 > read_limit;
    if hit_read_limit {
        raw.truncate(read_limit as usize);
    }

    let is_utf16_bom = raw.starts_with(&[0xFF, 0xFE]) || raw.starts_with(&[0xFE, 0xFF]);
    // 非 UTF-16 文本在解码前按最后一次 '\n' 截断原始字节：'\n' (0x0A) 绝不会作为
    // UTF-8 或 GB18030 多字节字符的后续字节出现，可防止截断点劈断多字节字符导致误判编码。
    if hit_read_limit && !is_utf16_bom {
        if let Some(pos) = raw.iter().rposition(|&b| b == b'\n') {
            raw.truncate(pos + 1);
        }
    }

    let mut document = Document::new(Kind::XLSX);
    let mut decoded = decode_text(&raw, hit_read_limit, &mut document)?;

    // UTF-16 截断后在解码文本中丢弃可能残缺的末行
    if hit_read_limit {
        if is_utf16_bom {
            if let Some(pos) = decoded.rfind('\n') {
                decoded.truncate(pos + 1);
            }
        }
        if scope == Scope::Full {
            document.warn("CSV file exceeds 16 MiB read limit; remaining content is truncated");
        }
    }

    let (delimiter, body) = resolve_delimiter(&decoded, kind);
    let max_rows = match scope {
        Scope::Full => MAX_ROWS,
        Scope::FirstViewport => FIRST_VIEWPORT_MAX_ROWS,
    };

    let sheet = build_sheet(
        sheet_name,
        body,
        delimiter,
        max_rows,
        scope == Scope::Full,
        &mut document,
    );

    let default_style = CellStyle {
        vertical_alignment: VerticalAlignment::Center,
        ..CellStyle::default()
    };
    let wrapped_style = CellStyle {
        wrap: true,
        vertical_alignment: VerticalAlignment::Center,
        ..CellStyle::default()
    };
    document.cell_styles = vec![default_style, wrapped_style];
    document.sheets.push(sheet);
    Ok(document)
}

fn sheet_name_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "Sheet1".to_owned())
}

/// 检查原始字节是否像二进制文件（非 UTF-16 却包含 NUL 字节）
fn validate_not_binary(bytes: &[u8]) -> Result<()> {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Ok(());
    }
    if bytes.contains(&0) {
        return Err(Error::Invalid("CSV file contains binary data"));
    }
    Ok(())
}

/// 按 BOM 与编码合法性将原始字节解码为 UTF-8 字符串：
/// UTF-8 BOM → UTF-16LE/BE BOM → 严格 UTF-8 → GB18030/GBK 回退
fn decode_text(
    bytes: &[u8],
    allow_truncated_tail: bool,
    document: &mut Document,
) -> Result<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        if rest.contains(&0) {
            return Err(Error::Invalid("CSV file contains binary data"));
        }
        return match std::str::from_utf8(rest) {
            Ok(s) => Ok(s.to_owned()),
            Err(err) if allow_truncated_tail && err.error_len().is_none() => {
                Ok(String::from_utf8_lossy(&rest[..err.valid_up_to()]).into_owned())
            }
            Err(_) => {
                document.warn("Invalid UTF-8 byte sequences were replaced");
                Ok(String::from_utf8_lossy(rest).into_owned())
            }
        };
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let even_len = if allow_truncated_tail {
            rest.len() & !1
        } else {
            rest.len()
        };
        let (decoded, had_errors) = crate::text_encoding::decode_utf16_le(&rest[..even_len]);
        if had_errors {
            document.warn("Invalid UTF-16LE byte sequences were replaced");
        }
        return Ok(decoded);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let even_len = if allow_truncated_tail {
            rest.len() & !1
        } else {
            rest.len()
        };
        let (decoded, had_errors) = crate::text_encoding::decode_utf16_be(&rest[..even_len]);
        if had_errors {
            document.warn("Invalid UTF-16BE byte sequences were replaced");
        }
        return Ok(decoded);
    }

    validate_not_binary(bytes)?;

    match std::str::from_utf8(bytes) {
        Ok(text) => return Ok(text.to_owned()),
        Err(err) if allow_truncated_tail && err.error_len().is_none() => {
            return Ok(String::from_utf8_lossy(&bytes[..err.valid_up_to()]).into_owned());
        }
        Err(_) => {}
    }

    let (cow, had_errors) = crate::text_encoding::decode_gb18030(bytes);
    if had_errors {
        document.warn("Non-UTF-8 characters were decoded with fallback replacements");
    }
    Ok(cow.into_owned())
}

/// 确定分隔符并返回待解析正文（若首行是 Excel 的 `sep=<char>` 指令则一并剥离）
fn resolve_delimiter(text: &str, kind: DelimitedKind) -> (u8, &str) {
    if kind == DelimitedKind::Tsv {
        return (b'\t', text);
    }
    if let Some((delim, rest)) = strip_excel_sep_directive(text) {
        return (delim, rest);
    }
    (detect_csv_delimiter(text), text)
}

/// 识别 Excel 导出的首行分隔符指令（如 `sep=,` / `sep=;` / `sep=\t`）
fn strip_excel_sep_directive(text: &str) -> Option<(u8, &str)> {
    let (first_line, rest) = match text.find('\n') {
        Some(pos) => (&text[..pos], &text[pos + 1..]),
        None => (text, ""),
    };
    let line = first_line.strip_suffix('\r').unwrap_or(first_line);
    let prefix = line.get(..4)?;
    if !prefix.eq_ignore_ascii_case("sep=") {
        return None;
    }
    let sep_part = &line[4..];
    if sep_part.len() == 1 {
        let b = sep_part.as_bytes()[0];
        if matches!(b, b',' | b';' | b'\t' | b'|') {
            return Some((b, rest));
        }
    }
    None
}

/// 通过抽样前 20 行引号外字符频次与一致性自动探测 `,` / `;` / `\t`
fn detect_csv_delimiter(text: &str) -> u8 {
    let candidates: [u8; 3] = [b',', b';', b'\t'];
    let mut counts: [Vec<usize>; 3] = [Vec::new(), Vec::new(), Vec::new()];

    let mut sampled_lines = 0_usize;
    let mut in_quotes = false;
    let mut line_counts = [0_usize; 3];
    let mut has_content = false;
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() && sampled_lines < 20 {
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
        match b {
            b'"' => {
                in_quotes = true;
                has_content = true;
            }
            b'\r' | b'\n' => {
                if has_content {
                    for idx in 0..3 {
                        counts[idx].push(line_counts[idx]);
                    }
                    sampled_lines += 1;
                }
                line_counts = [0; 3];
                has_content = false;
                if b == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 1;
                }
            }
            _ => {
                if !b.is_ascii_whitespace() || b == b'\t' {
                    has_content = true;
                }
                for (idx, &cand) in candidates.iter().enumerate() {
                    if b == cand {
                        line_counts[idx] += 1;
                    }
                }
            }
        }
        i += 1;
    }
    if has_content && sampled_lines < 20 {
        for idx in 0..3 {
            counts[idx].push(line_counts[idx]);
        }
    }

    // 评分策略：优先一致且非零的分隔符；若 `,` 已有一致且非零的出现则保持 `,` 优先
    let score = |samples: &[usize]| -> (usize, usize, usize) {
        if samples.is_empty() {
            return (0, 0, 0);
        }
        let non_zero: Vec<usize> = samples.iter().copied().filter(|&c| c > 0).collect();
        if non_zero.is_empty() {
            return (0, 0, 0);
        }
        let min_c = samples.iter().copied().min().unwrap_or(0);
        let max_c = samples.iter().copied().max().unwrap_or(0);
        let consistent = usize::from(min_c > 0 && min_c == max_c);
        let coverage = non_zero.len();
        let total: usize = non_zero.iter().sum();
        (consistent, coverage, total)
    };

    let comma_score = score(&counts[0]);
    if comma_score.0 == 1 {
        return b',';
    }
    let mut best_delim = b',';
    let mut best_score = comma_score;
    for idx in 1..3 {
        let s = score(&counts[idx]);
        if s > best_score {
            best_score = s;
            best_delim = candidates[idx];
        }
    }
    best_delim
}

fn build_sheet(
    name: &str,
    body: &str,
    delimiter: u8,
    max_rows: usize,
    emit_truncation_warnings: bool,
    document: &mut Document,
) -> Sheet {
    let mut cells: Vec<Cell> = Vec::new();
    let mut row_heights: Vec<f32> = Vec::new();
    let mut column_widths: Vec<f32> = Vec::new();

    let mut total_text_bytes = 0_usize;
    let mut truncated_rows = false;
    let mut truncated_columns = false;
    let mut truncated_cells = false;
    let mut truncated_text = false;

    let mut parser = CsvParser::new(body, delimiter);
    let mut row_buf: Vec<String> = Vec::new();
    let mut row_idx = 0_usize;

    while parser.next_row(&mut row_buf) {
        if row_idx >= max_rows {
            truncated_rows = true;
            break;
        }
        if row_buf.len() > MAX_COLUMNS {
            truncated_columns = true;
            row_buf.truncate(MAX_COLUMNS);
        }

        let mut row_height = DEFAULT_ROW_HEIGHT;
        let col_count = row_buf.len();
        if col_count > column_widths.len() {
            column_widths.resize(col_count, MIN_COLUMN_WIDTH);
        }

        for (col_idx, raw_field) in row_buf.drain(..).enumerate() {
            if raw_field.is_empty() {
                continue;
            }
            if cells.len() >= MAX_CELLS {
                truncated_cells = true;
                break;
            }
            let normalized = normalize_cell_newlines(&raw_field);
            let text = truncate_utf16(normalized.as_ref(), MAX_CELL_UTF16_LEN);

            if total_text_bytes.saturating_add(text.len()) > MAX_TEXT_BYTES {
                truncated_text = true;
                break;
            }
            total_text_bytes = total_text_bytes.saturating_add(text.len());

            let has_newline = text.contains('\n');
            if has_newline {
                let lines = text.split('\n').count();
                let h = (lines as f32 * LINE_HEIGHT + 6.0).min(MAX_ROW_HEIGHT);
                if h > row_height {
                    row_height = h;
                }
            }
            let est_width = estimate_cell_width(&text);
            if est_width > column_widths[col_idx] {
                column_widths[col_idx] = est_width.min(MAX_COLUMN_WIDTH);
            }

            let numeric = !has_newline && is_numeric(&text);
            let style = u32::from(has_newline);
            cells.push(Cell {
                row: row_idx as u32,
                column: col_idx as u32,
                text,
                style,
                numeric,
            });
        }

        row_heights.push(row_height);
        row_idx += 1;

        if truncated_cells || truncated_text {
            break;
        }
    }

    if emit_truncation_warnings {
        if truncated_rows {
            document.warn("CSV rows exceed 10000 limit; remaining rows are truncated");
        }
        if truncated_columns {
            document.warn("CSV columns exceed 256 limit; remaining columns are truncated");
        }
        if truncated_cells {
            document.warn("CSV cells exceed 50000 limit; remaining cells are truncated");
        }
        if truncated_text {
            document.warn("CSV text exceeds 8 MiB limit; remaining content is truncated");
        }
    }

    // 空文件兜底为 1x1 空白工作表，与 XLSX 空表行为保持一致
    if row_heights.is_empty() {
        row_heights.push(DEFAULT_ROW_HEIGHT);
    }
    if column_widths.is_empty() {
        column_widths.push(DEFAULT_COLUMN_WIDTH);
    }

    let mut sheet = Sheet {
        name: name.to_owned(),
        row_heights,
        column_widths,
        cells,
        merges: Vec::new(),
        elements: Vec::new(),
        frozen_rows: 0,
        frozen_columns: 0,
        geometry: None,
    };
    sheet.geometry = Some(SheetGeometry::compute(&sheet));
    sheet
}

fn normalize_cell_newlines(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    let replaced = text.replace("\r\n", "\n").replace('\r', "\n");
    Cow::Owned(replaced)
}

/// 按 UTF-16 码元上限截断超长单元格，对齐核心层与平台层安全契约
fn truncate_utf16(text: &str, max_units: usize) -> String {
    let mut units = 0_usize;
    for (byte_idx, ch) in text.char_indices() {
        let len = ch.len_utf16();
        if units + len > max_units {
            return text[..byte_idx].to_owned();
        }
        units += len;
    }
    text.to_owned()
}

/// 按每行显示宽度估算单元格推荐列宽（point）
fn estimate_cell_width(text: &str) -> f32 {
    let mut max_line_pt = 0.0_f32;
    for line in text.split('\n') {
        let mut width = 0.0_f32;
        for ch in line.chars() {
            width += if is_east_asian_char(ch) { 11.5 } else { 6.5 };
            if width >= MAX_COLUMN_WIDTH {
                return MAX_COLUMN_WIDTH;
            }
        }
        if width > max_line_pt {
            max_line_pt = width;
        }
    }
    (max_line_pt + 14.0).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
}

/// 判断单元格文本是否为数值（用于 General 对齐下自动右对齐）
///
/// 排除前导零编码（如 `00123` 邮编/工号）、`NaN` / `inf`，支持可选正负号、
/// 千分位逗号（如 `1,234.56`）、科学计数法及尾随百分号（如 `12.5%`）。
fn is_numeric(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let without_pct = trimmed.strip_suffix('%').unwrap_or(trimmed).trim_end();
    if without_pct.is_empty() {
        return false;
    }
    let unsigned = without_pct
        .strip_prefix('+')
        .or_else(|| without_pct.strip_prefix('-'))
        .unwrap_or(without_pct);
    if unsigned.is_empty() {
        return false;
    }

    // 剥离合法的千分位逗号（仅在整数部分存在且符合 d{1,3}(,ddd)+ 模式时接受）
    let candidate: Cow<'_, str> = if unsigned.contains(',') {
        let int_part = unsigned.split(['.', 'e', 'E']).next().unwrap_or("");
        let groups: Vec<&str> = int_part.split(',').collect();
        let valid_groups = groups.len() >= 2
            && !groups[0].is_empty()
            && groups[0].len() <= 3
            && groups[0].bytes().all(|b| b.is_ascii_digit())
            && groups[1..]
                .iter()
                .all(|g| g.len() == 3 && g.bytes().all(|b| b.is_ascii_digit()));
        if !valid_groups {
            return false;
        }
        Cow::Owned(unsigned.replace(',', ""))
    } else {
        Cow::Borrowed(unsigned)
    };

    // 排除前导零编码（如 "00123"），允许 "0"、"0.5"、"0e1"
    if candidate.starts_with('0') && candidate.len() > 1 {
        let second = candidate.as_bytes()[1];
        if second.is_ascii_digit() {
            return false;
        }
    }

    // 仅允许 ASCII 数字及小数点/科学计数法，拒绝 "NaN" / "inf"
    if !candidate.bytes().any(|b| b.is_ascii_digit()) {
        return false;
    }
    candidate.parse::<f64>().is_ok_and(f64::is_finite)
}

/// 逐行流式 RFC 4180 CSV 解析器
struct CsvParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    delimiter: u8,
}

impl<'a> CsvParser<'a> {
    fn new(text: &'a str, delimiter: u8) -> Self {
        Self {
            bytes: text.as_bytes(),
            pos: 0,
            delimiter,
        }
    }

    fn next_row(&mut self, out: &mut Vec<String>) -> bool {
        out.clear();
        if self.pos >= self.bytes.len() {
            return false;
        }

        loop {
            let field = self.read_field();
            out.push(field);

            if self.pos >= self.bytes.len() {
                break;
            }
            let b = self.bytes[self.pos];
            if b == self.delimiter {
                self.pos += 1;
                // 行尾分隔符后紧跟 EOF 或换行时，仍需补齐最后一个空单元格
                if self.pos >= self.bytes.len() {
                    out.push(String::new());
                    break;
                }
                continue;
            }
            if b == b'\r' {
                self.pos += 1;
                if self.pos < self.bytes.len() && self.bytes[self.pos] == b'\n' {
                    self.pos += 1;
                }
                break;
            }
            if b == b'\n' {
                self.pos += 1;
                break;
            }
            break;
        }

        true
    }

    fn read_field(&mut self) -> String {
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'"' {
            self.read_quoted_field()
        } else {
            self.read_unquoted_field()
        }
    }

    fn read_quoted_field(&mut self) -> String {
        // 跳过起始双引号
        self.pos += 1;
        let mut buf: Vec<u8> = Vec::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                if self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1] == b'"' {
                    buf.push(b'"');
                    self.pos += 2;
                    continue;
                }
                self.pos += 1;
                // 容错：闭合引号后到分隔符/换行前的尾随空白或字符按原样吸收，避免死循环
                while self.pos < self.bytes.len() {
                    let next = self.bytes[self.pos];
                    if next == self.delimiter || next == b'\r' || next == b'\n' {
                        break;
                    }
                    buf.push(next);
                    self.pos += 1;
                }
                break;
            }
            buf.push(b);
            self.pos += 1;
        }
        String::from_utf8(buf)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }

    fn read_unquoted_field(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == self.delimiter || b == b'\r' || b == b'\n' {
                break;
            }
            self.pos += 1;
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .map(ToOwned::to_owned)
            .unwrap_or_else(|_| String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned())
    }
}
