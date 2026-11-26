use super::flag;
use super::geometry::{points, Bounds};
use super::render::Render;
use super::table_style::{
    parse_line, resolve_table_style, Edge, EDGE_BOTTOM, EDGE_LEFT, EDGE_RIGHT, EDGE_TOP,
};
use crate::model::{CellBorder, CellRange, Document, Element, ElementType, Run};
use crate::xml::Node;
use crate::{Error, Result};

/// DrawingML 单元格缺省内边距（`marT` / `marR` / `marB` / `marL`），与 DOCX 的 5pt 默认值不同。
const DEFAULT_PADDING_TOP: f32 = 3.6;
const DEFAULT_PADDING_RIGHT: f32 = 7.2;
const DEFAULT_PADDING_BOTTOM: f32 = 3.6;
const DEFAULT_PADDING_LEFT: f32 = 7.2;
/// 平台层统一默认内边距；全表等于该值时不输出 `cell_paddings`。
const PLATFORM_PADDING: [f32; 4] = [5.0, 5.0, 5.0, 5.0];

impl Render<'_> {
    pub fn graphic(
        &mut self,
        part: &super::parts::Part,
        shape: &Node,
        bounds: Bounds,
    ) -> Result<Vec<Element>> {
        let data = shape.child("graphic").and_then(|n| n.child("graphicData"));
        if let Some(chart) = data.and_then(|n| n.child("chart")) {
            return self.chart(part, chart, bounds);
        }
        if let Some(ole) = data.and_then(|n| {
            n.child("oleObj").or_else(|| {
                n.named("AlternateContent").find_map(|ac| {
                    super::unsupported::select_alternate(ac)
                        .and_then(|branch| branch.node.child("oleObj"))
                })
            })
        }) {
            if let Some(element) = ole
                .child("pic")
                .and_then(|pic| self.picture(part, pic, bounds))
            {
                self.doc
                    .warn("PPTX embedded OLE objects display their saved preview pictures.");
                return Ok(vec![element]);
            }
            self.doc
                .warn("PPTX multimedia and embedded objects are not supported.");
            return Ok(Vec::new());
        }
        let Some(table) = data.and_then(|n| n.child("tbl")) else {
            match data.map_or("", |n| n.attr("uri")) {
                uri if uri.contains("diagram") => self.doc.warn("PPTX SmartArt is omitted."),
                _ => self
                    .doc
                    .warn("Unsupported PPTX graphic frames are omitted."),
            }
            return Ok(Vec::new());
        };
        let Some(grid) = table.child("tblGrid") else {
            self.doc
                .warn("PPTX tables with missing column widths are omitted.");
            return Ok(Vec::new());
        };
        let mut element = bounds.element(ElementType::TABLE);
        for column in grid.named("gridCol") {
            if element.column_widths.len() >= 32 {
                return Err(Error::Limit("PPTX table exceeds 32 columns"));
            }
            let width = points(column.attr("w"))
                .filter(|w| *w > 0.0)
                .ok_or(Error::Invalid("Invalid PPTX table column width"))?;
            element.column_widths.push(width);
        }
        if element.column_widths.is_empty() {
            return Err(Error::Invalid("PPTX table has no columns"));
        }
        let total_weight: f32 = element.column_widths.iter().map(|w| w.max(1.0)).sum();
        if total_weight > 0.0 && element.width > 0.0 {
            let table_width = element.width;
            for width in &mut element.column_widths {
                *width = table_width * width.max(1.0) / total_weight;
            }
        }
        let cols = element.column_widths.len();
        let mut any_valign = false;
        let mut merge_flags = Vec::new();
        // 行高与内边距按行收集，稍后统一收敛为可选输出字段
        let mut row_heights: Vec<f32> = Vec::new();
        let mut cell_paddings: Vec<Vec<[f32; 4]>> = Vec::new();
        let mut any_diagonal_or_3d = false;
        // 先统计行数以解析表样式，使每个单元格正文只解析一次（tcTxStyle 作为初始基线直接注入），
        // 避免"先按默认基线解析、再整格重解析"造成解析翻倍与文本预算被重复计入。
        let row_count_estimate = table.named("tr").take(501).count();
        if row_count_estimate > 500 {
            return Err(Error::Limit("PPTX table exceeds 500 rows"));
        }
        let resolved = resolve_table_style(
            table,
            row_count_estimate,
            cols,
            self.table_styles.map(|part| &part.root),
            self.theme,
            self.budget,
            self.doc,
        );
        for row in table.named("tr") {
            if element.rows.len() >= 500 {
                return Err(Error::Limit("PPTX table exceeds 500 rows"));
            }
            // <a:tr h="..."> 为显式行高（EMU）；无效或非正值写 0，平台层以 max(20, h) 兜底
            row_heights.push(
                points(row.attr("h"))
                    .filter(|h| *h > 0.0)
                    .map(|h| h.clamp(0.0, 14400.0))
                    .unwrap_or(0.0),
            );
            let mut cells = Vec::new();
            let mut fills = Vec::new();
            let mut valigns = Vec::new();
            let mut row_merge_flags = Vec::new();
            let mut row_paddings = Vec::new();
            // 本行是否存在显式 anchor；仅当任一单元格声明了 anchor 才输出该行
            let mut has_valign = false;
            for cell in row.named("tc") {
                if cells.len() >= cols {
                    return Err(Error::Invalid("PPTX table is not rectangular"));
                }
                self.budget.object()?;
                let grid_span = merge_span(cell.attr("gridSpan"))?;
                let row_span = merge_span(cell.attr("rowSpan"))?;
                let h_merge = flag(cell.attr("hMerge"), false);
                let v_merge = flag(cell.attr("vMerge"), false);
                row_merge_flags.push((h_merge, v_merge));
                // 合并跨度只在锚点单元格声明；hMerge / vMerge 用于标记被覆盖的位置。
                if !h_merge && !v_merge && (grid_span > 1 || row_span > 1) {
                    let start_row = element.rows.len() as u32;
                    let start_column = cells.len() as u32;
                    element.merges.push(CellRange {
                        start_row,
                        start_column,
                        end_row: start_row.saturating_add(row_span - 1),
                        end_column: start_column.saturating_add(grid_span - 1),
                    });
                }
                let cell_row = element.rows.len();
                let cell_col = cells.len();
                let paragraphs = match cell.child("txBody") {
                    Some(body) => {
                        let initial = self.cell_initial_run(&resolved, cell_row, cell_col, body);
                        self.paragraphs_with_base(body, &[], initial)?
                    }
                    None => Vec::new(),
                };
                cells.push(paragraphs);
                let properties = cell.child("tcPr");
                let fill = match properties {
                    Some(properties) => self.theme.fill(properties, 0, self.doc),
                    None => 0,
                };
                fills.push(fill);
                // <a:tcPr anchor="t|ctr|b"> 映射为 0 顶部 / 1 居中 / 2 底部
                let align = properties.and_then(|p| match p.attr("anchor") {
                    "t" => Some(0u8),
                    "ctr" => Some(1),
                    "b" => Some(2),
                    _ => None,
                });
                if align.is_some() {
                    has_valign = true;
                }
                valigns.push(align.unwrap_or(1));
                row_paddings.push(cell_padding(properties));
                if let Some(properties) = properties {
                    // 对角线边框与 3D 效果仍未还原；直设 lnL/lnR/lnT/lnB 与 mar* 已支持
                    if ["lnTlToBr", "lnBlToTr", "cell3D"]
                        .iter()
                        .any(|name| properties.child(name).is_some())
                    {
                        any_diagonal_or_3d = true;
                    }
                }
            }
            if cells.len() != cols {
                return Err(Error::Invalid("PPTX table is not rectangular"));
            }
            element.rows.push(cells);
            element.cell_fills.push(fills);
            element.cell_valigns.push(valigns);
            cell_paddings.push(row_paddings);
            merge_flags.push(row_merge_flags);
            if has_valign {
                any_valign = true;
            }
        }
        // 没有任何显式 anchor 时保持 cellValigns 为空，平台层沿用垂直居中
        if !any_valign {
            element.cell_valigns.clear();
        }
        if element.rows.is_empty() {
            return Ok(Vec::new());
        }
        // 表样式级联：填充、四边边框（文本基线已在单元格解析时注入）
        self.apply_table_styles(&mut element, table, &resolved)?;
        resolve_merges(&mut element, &merge_flags, self.doc)?;
        if any_diagonal_or_3d {
            self.doc
                .warn("PPTX diagonal cell borders and 3D effects are omitted.");
        }
        // 行高：仅当至少存在一行有效显式行高时输出
        if row_heights.iter().any(|&h| h > 0.0) {
            element.row_heights = row_heights;
        }
        // 内边距：仅当存在任意格不为平台默认 [5, 5, 5, 5] 时输出
        if cell_paddings
            .iter()
            .flatten()
            .any(|padding| *padding != PLATFORM_PADDING)
        {
            element.cell_paddings = cell_paddings;
        }
        Ok(vec![element])
    }
}

/// 解析 `<a:tcPr>` 属性上的四边内边距 `[top, right, bottom, left]`。
///
/// `mar*` 是属性而非子元素；缺省时使用 DrawingML 默认值，单边钳位 `[0.0, 200.0]`。
fn cell_padding(properties: Option<&Node>) -> [f32; 4] {
    let Some(properties) = properties else {
        return [
            DEFAULT_PADDING_TOP,
            DEFAULT_PADDING_RIGHT,
            DEFAULT_PADDING_BOTTOM,
            DEFAULT_PADDING_LEFT,
        ];
    };
    let read = |name: &str, default: f32| {
        points(properties.attr(name))
            .filter(|v| *v >= 0.0)
            .map(|v| v.clamp(0.0, 200.0))
            .unwrap_or(default)
    };
    [
        read("marT", DEFAULT_PADDING_TOP),
        read("marR", DEFAULT_PADDING_RIGHT),
        read("marB", DEFAULT_PADDING_BOTTOM),
        read("marL", DEFAULT_PADDING_LEFT),
    ]
}

impl Render<'_> {
    /// 构造单元格正文解析的初始 `Run` 基线。
    ///
    /// 以表样式 `tcTxStyle` 的粗体、斜体、字色与主题字体为底；单元格自身的
    /// `lstStyle` / `pPr/defRPr` / `rPr` 仍会在 `Text::read_with_base` 内覆盖该基线，
    /// 因此单元格直设文本样式优先级高于表样式。
    fn cell_initial_run(
        &self,
        resolved: &super::table_style::ResolvedTableStyles,
        row: usize,
        col: usize,
        body: &Node,
    ) -> Run {
        let defaults = resolved.texts.get(row).and_then(|cells| cells.get(col));
        let mut initial = Run {
            size: 18.0,
            color: defaults
                .and_then(|d| d.color)
                .unwrap_or_else(|| self.theme.text_color()),
            ..Run::default()
        };
        let Some(defaults) = defaults else {
            return initial;
        };
        if let Some(bold) = defaults.bold {
            initial.bold = bold;
        }
        if let Some(italic) = defaults.italic {
            initial.italic = italic;
        }
        // 主题字体：按单元格是否含东亚文字选择 ea / latin 字族
        if let Some(key) = defaults.font_key.as_deref() {
            let east_asian = body_has_east_asian(body);
            if let Some(face) = self.theme.font_face(key, east_asian) {
                initial.font_face = face.to_owned();
            }
        }
        initial
    }

    /// 把表样式级联结果写入元素：填充、`tcTxStyle` 文本基线、`<a:tcPr>` 直设覆盖与边框收敛。
    fn apply_table_styles(
        &mut self,
        element: &mut Element,
        table: &Node,
        resolved: &super::table_style::ResolvedTableStyles,
    ) -> Result<()> {
        let rows = element.rows.len();
        let cols = element.column_widths.len();
        let mut edges = resolved.borders.clone();
        // 记录哪些边由 `<a:tcPr>` 直设，避免共享邻边被反向覆盖
        let mut direct = vec![vec![[false; 4]; cols]; rows];

        for (r, row) in table.named("tr").enumerate() {
            for (c, cell) in row.named("tc").enumerate() {
                if r >= rows || c >= cols {
                    continue;
                }
                let properties = cell.child("tcPr");
                // 填充：仅当 tcPr 含显式填充子树时才覆盖级联底纹，否则保留表样式底纹
                let explicit_fill = properties.is_some_and(|p| {
                    [
                        "noFill",
                        "solidFill",
                        "gradFill",
                        "pattFill",
                        "blipFill",
                        "grpFill",
                    ]
                    .iter()
                    .any(|name| p.child(name).is_some())
                });
                if explicit_fill {
                    if let Some(properties) = properties {
                        element.cell_fills[r][c] = self.theme.fill(properties, 0, self.doc);
                    }
                } else {
                    element.cell_fills[r][c] = resolved.fills[r][c].unwrap_or(0);
                }

                // 直设四边边框：lnT / lnR / lnB / lnL
                if let Some(properties) = properties {
                    for (index, name) in [
                        (EDGE_TOP, "lnT"),
                        (EDGE_RIGHT, "lnR"),
                        (EDGE_BOTTOM, "lnB"),
                        (EDGE_LEFT, "lnL"),
                    ] {
                        let Some(line) = properties.child(name) else {
                            continue;
                        };
                        // parse_line 只会返回 Hidden 或 Line，不会返回 Unset
                        edges[r][c][index] = parse_line(line, self.theme, self.doc);
                        direct[r][c][index] = true;
                    }
                }
            }
        }

        // 直设边框同步到共享邻边，确保邻格重绘时不遮盖直设样式或 noFill
        for r in 0..rows {
            for c in 0..cols {
                if direct[r][c][EDGE_RIGHT] && c + 1 < cols && !direct[r][c + 1][EDGE_LEFT] {
                    edges[r][c + 1][EDGE_LEFT] = edges[r][c][EDGE_RIGHT];
                }
                if direct[r][c][EDGE_LEFT] && c > 0 && !direct[r][c - 1][EDGE_RIGHT] {
                    edges[r][c - 1][EDGE_RIGHT] = edges[r][c][EDGE_LEFT];
                }
                if direct[r][c][EDGE_BOTTOM] && r + 1 < rows && !direct[r + 1][c][EDGE_TOP] {
                    edges[r + 1][c][EDGE_TOP] = edges[r][c][EDGE_BOTTOM];
                }
                if direct[r][c][EDGE_TOP] && r > 0 && !direct[r - 1][c][EDGE_BOTTOM] {
                    edges[r - 1][c][EDGE_BOTTOM] = edges[r][c][EDGE_TOP];
                }
            }
        }

        // 合并区域外框归并：锚点右上取末列边、下边取末行边
        for merge in &element.merges {
            let (sr, sc) = (merge.start_row as usize, merge.start_column as usize);
            let (er, ec) = (merge.end_row as usize, merge.end_column as usize);
            if sr >= rows || sc >= cols || er >= rows || ec >= cols {
                continue;
            }
            edges[sr][sc][EDGE_RIGHT] = edges[sr][ec][EDGE_RIGHT];
            edges[sr][sc][EDGE_BOTTOM] = edges[er][sc][EDGE_BOTTOM];
        }

        apply_border_output(element, &edges, resolved.has_any_border_rule);
        Ok(())
    }
}

/// 把 `Edge` 矩阵收敛成最少量的输出字段。
///
/// - 全 `Unset`：保持 `cell_borders` 为空，平台层沿用默认网格；
/// - 不存在 `Hidden` 且所有可见边颜色线宽一致：写入元素级 `stroke` / `stroke_width`；
/// - 其余情况输出完整 `rows × cols × 4` 的 `cell_borders`。
fn apply_border_output(element: &mut Element, edges: &[Vec<[Edge; 4]>], has_any_border_rule: bool) {
    let rows = edges.len();
    let cols = edges.first().map_or(0, Vec::len);
    let mut any_set = has_any_border_rule;
    let mut any_hidden = false;
    // 统一描边收敛要求全表不存在未声明边：只要有一格某条边是 Unset，
    // 就无法用单一线宽表达该表，必须回退到逐格 cell_borders。
    let mut any_unset = false;
    let mut uniform: Option<CellBorder> = None;
    let mut uniform_holds = true;

    for row in edges {
        for cell in row {
            for edge in cell {
                match edge {
                    Edge::Unset => {
                        any_unset = true;
                        uniform_holds = false;
                    }
                    Edge::Hidden => {
                        any_set = true;
                        any_hidden = true;
                        uniform_holds = false;
                    }
                    Edge::Line(border) => {
                        any_set = true;
                        match uniform {
                            None => uniform = Some(*border),
                            Some(existing) => {
                                if existing != *border {
                                    uniform_holds = false;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if !any_set {
        // 全表无任何边框声明，沿用平台默认 #b7bec7 网格
        return;
    }
    if uniform_holds && !any_hidden && !any_unset {
        if let Some(border) = uniform {
            element.stroke = border.color;
            element.stroke_width = border.width;
            return;
        }
    }
    // 存在 Hidden 或颜色线宽不一致：输出完整矩阵，Hidden 与 Unset 均写零宽
    let mut output = Vec::with_capacity(rows);
    for row in edges {
        let mut output_row = Vec::with_capacity(cols);
        for cell in row {
            let mut mapped = [CellBorder::default(); 4];
            for (index, edge) in cell.iter().enumerate() {
                if let Edge::Line(border) = edge {
                    mapped[index] = *border;
                }
            }
            output_row.push(mapped);
        }
        output.push(output_row);
    }
    element.cell_borders = output;
}

/// 判断正文中是否出现东亚文字，用于在主题 `fontScheme` 中挑选 `ea` 或 `latin` 字族。
///
/// 复用 `model` 层的统一判定，覆盖 CJK 表意文字、假名、谚文与全角符号，
/// 避免本地重复维护字符范围时漏掉韩文（U+AC00..U+D7AF）等区段。
fn body_has_east_asian(body: &Node) -> bool {
    body.named("p").any(|paragraph| {
        paragraph.named("r").any(|run| {
            run.named("t")
                .any(|segment| crate::model::contains_east_asian(&segment.text))
        })
    })
}

fn merge_span(value: &str) -> Result<u32> {
    if value.is_empty() {
        return Ok(1);
    }
    value
        .parse::<u32>()
        .ok()
        .filter(|span| *span > 0)
        .ok_or(Error::Invalid("Invalid PPTX table merge span"))
}

fn resolve_merges(
    element: &mut Element,
    merge_flags: &[Vec<(bool, bool)>],
    document: &mut Document,
) -> Result<()> {
    let row_count = element.rows.len();
    let column_count = element.column_widths.len();
    let mut covered = vec![false; row_count * column_count];
    let mut missing_continuation_flags = false;

    for merge_index in 0..element.merges.len() {
        let merge = element.merges[merge_index];
        if merge.end_row as usize >= row_count || merge.end_column as usize >= column_count {
            return Err(Error::Invalid("PPTX table merge exceeds grid"));
        }
        for row in merge.start_row..=merge.end_row {
            for column in merge.start_column..=merge.end_column {
                let index = row as usize * column_count + column as usize;
                if covered[index] {
                    return Err(Error::Invalid("Overlapping PPTX table merged ranges"));
                }
                covered[index] = true;
                if row == merge.start_row && column == merge.start_column {
                    continue;
                }
                let (h_merge, v_merge) = merge_flags[row as usize][column as usize];
                if !h_merge && !v_merge {
                    missing_continuation_flags = true;
                }
            }
        }
    }

    let orphan_continuation = merge_flags.iter().enumerate().any(|(row, flags)| {
        flags
            .iter()
            .enumerate()
            .any(|(column, (h_merge, v_merge))| {
                (*h_merge || *v_merge) && !covered[row * column_count + column]
            })
    });
    if missing_continuation_flags || orphan_continuation {
        document.warn("PPTX table merge flags are inconsistent; declared spans are used.");
    }

    // 从属位置不再保留重复正文，绘制、检索和选区都只读取合并区域锚点。
    for merge_index in 0..element.merges.len() {
        let merge = element.merges[merge_index];
        for row in merge.start_row..=merge.end_row {
            for column in merge.start_column..=merge.end_column {
                if row != merge.start_row || column != merge.start_column {
                    element.rows[row as usize][column as usize].clear();
                }
            }
        }
    }
    Ok(())
}
