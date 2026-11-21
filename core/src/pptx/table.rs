use super::flag;
use super::geometry::{points, Bounds};
use super::render::Render;
use crate::model::{CellRange, Document, Element, ElementType};
use crate::xml::Node;
use crate::{Error, Result};

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
        let mut any_valign = false;
        let mut merge_flags = Vec::new();
        for row in table.named("tr") {
            if element.rows.len() >= 500 {
                return Err(Error::Limit("PPTX table exceeds 500 rows"));
            }
            let mut cells = Vec::new();
            let mut fills = Vec::new();
            let mut valigns = Vec::new();
            let mut row_merge_flags = Vec::new();
            // 本行是否存在显式 anchor；仅当任一单元格声明了 anchor 才输出该行
            let mut has_valign = false;
            for cell in row.named("tc") {
                if cells.len() >= element.column_widths.len() {
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
                let paragraphs = match cell.child("txBody") {
                    Some(body) => self.paragraphs(body, &[])?,
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
                if let Some(properties) = properties {
                    // 边框与页边距仍未还原；垂直对齐已支持，故不再把它计入警告
                    if ["lnL", "lnR", "lnT", "lnB", "lnTlToBr", "lnBlToTr", "cell3D"]
                        .iter()
                        .any(|name| properties.child(name).is_some())
                        || properties.child("marL").is_some()
                        || properties.child("marR").is_some()
                        || properties.child("marT").is_some()
                        || properties.child("marB").is_some()
                    {
                        self.doc
                            .warn("PPTX cell borders and margins are simplified.");
                    }
                }
            }
            if cells.len() != element.column_widths.len() {
                return Err(Error::Invalid("PPTX table is not rectangular"));
            }
            element.rows.push(cells);
            element.cell_fills.push(fills);
            element.cell_valigns.push(valigns);
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
        resolve_merges(&mut element, &merge_flags, self.doc)?;
        self.doc.warn("PPTX table row heights are approximate.");
        if table.child("tblPr").is_some() {
            self.doc.warn("PPTX table styles are simplified.");
        }
        Ok(vec![element])
    }
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
