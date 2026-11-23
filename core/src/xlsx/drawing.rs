use super::{coordinates, unsigned, Workbook, MAX_COLUMNS, MAX_ROWS};
use crate::model::{Document, Sheet};
use crate::pptx::geometry::points;
use crate::xml::Node;
use crate::{Error, Package, Result};

const MAX_CHARTS_PER_SHEET: usize = 128;
const MAX_COORDINATE: f32 = 100_000.0;

#[derive(Clone, Copy)]
struct Frame {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy)]
enum ExtentPlan {
    /// 目标至少覆盖到的行列数（含 0-based 锚点所在行列），由 `twoCellAnchor` 使用。
    Markers { rows: usize, columns: usize },
    /// 至少扩展到 `min_rows`/`min_columns` 后，再按 `max(1.0)` 步长延伸到 `right`/`bottom`。
    Extent {
        min_rows: usize,
        min_columns: usize,
        right: f32,
        bottom: f32,
    },
}

#[derive(Clone, Copy)]
struct Marker {
    row: u32,
    column: u32,
    row_offset: f32,
    column_offset: f32,
}

pub(super) struct GridDefaults {
    pub row_height: f32,
    pub column_width: f32,
}

/// 解析 Spreadsheet Drawing，只提取其中的图表引用并交给跨格式共享图表引擎。
pub(super) fn parse(
    package: &mut Package,
    source: &str,
    worksheet: &Node,
    sheet: &mut Sheet,
    defaults: GridDefaults,
    workbook: &mut Workbook,
    document: &mut Document,
) -> Result<()> {
    let Some(reference) = worksheet.child("drawing") else {
        return Ok(());
    };
    let relationships = package.relationships(source)?;
    let Some(target) = relationships
        .get(crate::pptx::relation_id(reference, "id"))
        .filter(|path| package.has(path))
        .cloned()
    else {
        document.warn("XLSX drawings with missing or external parts were omitted.");
        return Ok(());
    };
    let root = package.xml(&target)?;
    if root.name != "wsDr" {
        return Err(Error::Invalid("Invalid XLSX drawing part"));
    }
    let drawing_relationships = package.relationships(&target)?;
    let mut chart_count = 0_usize;
    let mut unsupported = false;
    let mut anchors = Vec::new();
    collect_anchors(&root.children, &mut anchors, document);
    for anchor in anchors {
        let Some(chart) = anchor.descendant("chart") else {
            unsupported = true;
            continue;
        };
        chart_count += 1;
        if chart_count > MAX_CHARTS_PER_SHEET {
            return Err(Error::Limit("XLSX worksheet exceeds 128 charts"));
        }
        // 只读推算定位与扩展计划；确认图表部件有效且完成渲染后再提交行列扩展，避免无效图表撑大工作表。
        let Some((frame, extent_plan)) =
            resolve_frame(anchor, sheet, defaults.row_height, defaults.column_width)?
        else {
            document.warn("XLSX charts with invalid or empty anchors were omitted.");
            continue;
        };
        let Some(chart_path) = drawing_relationships
            .get(crate::pptx::relation_id(chart, "id"))
            .filter(|path| package.has(path))
            .cloned()
        else {
            document.warn("XLSX charts with missing or external parts were omitted.");
            continue;
        };
        let chart_root = package.xml(&chart_path)?;
        let theme = &workbook.theme;
        let budget = &mut workbook.budget;
        let Some(mut elements) = crate::chart::render(
            &chart_root,
            frame.width,
            frame.height,
            theme,
            document,
            &mut |bytes| budget.text_bytes(bytes),
        )?
        else {
            continue;
        };
        workbook.budget.elements(elements.len())?;
        apply_extent(
            sheet,
            extent_plan,
            defaults.row_height,
            defaults.column_width,
        );
        for element in &mut elements {
            // 共享引擎输出图表局部坐标；Sheet 与 PPTX 一样通过 transform 平移到目标坐标系。
            element.x += frame.x;
            element.y += frame.y;
            element.transform = crate::pptx::drawing::element_transform(element);
        }
        sheet.elements.extend(elements);
    }
    if unsupported {
        document.warn("Worksheet drawings other than charts are not rendered");
    }
    Ok(())
}

fn collect_anchors<'a>(nodes: &'a [Node], out: &mut Vec<&'a Node>, document: &mut Document) {
    for node in nodes {
        match node.name.as_str() {
            "twoCellAnchor" | "oneCellAnchor" | "absoluteAnchor" => out.push(node),
            "AlternateContent" => {
                if let Some((branch, lossless)) = select_alternate(node) {
                    if !lossless {
                        document.warn(
                            "XLSX alternate content uses one saved representation; unsupported features may be lost.",
                        );
                    }
                    collect_anchors(&branch.children, out, document);
                } else {
                    document.warn("XLSX alternate content is omitted.");
                }
            }
            _ => (),
        }
    }
}

fn select_alternate(node: &Node) -> Option<(&Node, bool)> {
    for choice in node.named("Choice") {
        let supported = choice
            .attr("Requires")
            .split_ascii_whitespace()
            .all(|prefix| matches!(prefix, "xdr" | "a" | "r" | "c"));
        if supported {
            return Some((choice, true));
        }
    }
    node.child("Fallback").map(|fallback| (fallback, false))
}

fn resolve_frame(
    anchor: &Node,
    sheet: &Sheet,
    default_row_height: f32,
    default_column_width: f32,
) -> Result<Option<(Frame, ExtentPlan)>> {
    let (frame, plan) = match anchor.name.as_str() {
        "twoCellAnchor" => {
            let (Some(from), Some(to)) = (anchor.child("from"), anchor.child("to")) else {
                return Ok(None);
            };
            let from = marker(from)?;
            let to = marker(to)?;
            let rows = marker_len(from.row)?.max(marker_len(to.row)?);
            let columns = marker_len(from.column)?.max(marker_len(to.column)?);
            let (x, y) =
                marker_position_readonly(sheet, from, default_row_height, default_column_width);
            let (right, bottom) =
                marker_position_readonly(sheet, to, default_row_height, default_column_width);
            (
                Frame {
                    x,
                    y,
                    width: right - x,
                    height: bottom - y,
                },
                ExtentPlan::Markers { rows, columns },
            )
        }
        "oneCellAnchor" => {
            let (Some(from), Some(extent)) = (anchor.child("from"), anchor.child("ext")) else {
                return Ok(None);
            };
            let from = marker(from)?;
            let min_rows = marker_len(from.row)?;
            let min_columns = marker_len(from.column)?;
            let (x, y) =
                marker_position_readonly(sheet, from, default_row_height, default_column_width);
            let (Some(width), Some(height)) =
                (points(extent.attr("cx")), points(extent.attr("cy")))
            else {
                return Ok(None);
            };
            (
                Frame {
                    x,
                    y,
                    width,
                    height,
                },
                ExtentPlan::Extent {
                    min_rows,
                    min_columns,
                    right: x + width,
                    bottom: y + height,
                },
            )
        }
        "absoluteAnchor" => {
            let (Some(position), Some(extent)) = (anchor.child("pos"), anchor.child("ext")) else {
                return Ok(None);
            };
            let (Some(x), Some(y), Some(width), Some(height)) = (
                points(position.attr("x")),
                points(position.attr("y")),
                points(extent.attr("cx")),
                points(extent.attr("cy")),
            ) else {
                return Ok(None);
            };
            (
                Frame {
                    x,
                    y,
                    width,
                    height,
                },
                ExtentPlan::Extent {
                    min_rows: 0,
                    min_columns: 0,
                    right: x + width,
                    bottom: y + height,
                },
            )
        }
        _ => return Ok(None),
    };
    if !(frame.x >= 0.0
        && frame.y >= 0.0
        && frame.width > 0.0
        && frame.height > 0.0
        && frame.x + frame.width <= MAX_COORDINATE
        && frame.y + frame.height <= MAX_COORDINATE)
    {
        return Ok(None);
    }
    if let ExtentPlan::Extent {
        min_rows,
        min_columns,
        right,
        bottom,
    } = plan
    {
        if !can_cover_extent(
            sheet.row_heights.len(),
            sheet.row_heights.iter().sum::<f32>(),
            min_rows,
            bottom,
            default_row_height,
            MAX_ROWS as usize,
        ) || !can_cover_extent(
            sheet.column_widths.len(),
            sheet.column_widths.iter().sum::<f32>(),
            min_columns,
            right,
            default_column_width,
            MAX_COLUMNS as usize,
        ) {
            return Ok(None);
        }
    }
    Ok(Some((frame, plan)))
}

fn marker(node: &Node) -> Result<Marker> {
    let (Some(column), Some(row), Some(column_offset), Some(row_offset)) = (
        node.child("col"),
        node.child("row"),
        node.child("colOff"),
        node.child("rowOff"),
    ) else {
        return Err(Error::Invalid("Invalid XLSX drawing marker"));
    };
    let column = zero_based(unsigned(column.text.trim())?, false)?;
    let row = zero_based(unsigned(row.text.trim())?, true)?;
    let (Some(column_offset), Some(row_offset)) = (
        points(column_offset.text.trim()),
        points(row_offset.text.trim()),
    ) else {
        return Err(Error::Invalid("Invalid XLSX drawing marker offset"));
    };
    Ok(Marker {
        row,
        column,
        row_offset,
        column_offset,
    })
}

fn zero_based(value: u32, row: bool) -> Result<u32> {
    let one_based = value
        .checked_add(1)
        .ok_or(Error::Limit("XLSX drawing anchor exceeds sheet limits"))?;
    if row {
        coordinates::row_index(one_based)
    } else {
        coordinates::column_index(one_based)
    }
}

fn marker_len(index: u32) -> Result<usize> {
    usize::try_from(index + 1).map_err(|_| Error::Limit("XLSX drawing anchor exceeds sheet limits"))
}

fn marker_position_readonly(
    sheet: &Sheet,
    marker: Marker,
    default_row_height: f32,
    default_column_width: f32,
) -> (f32, f32) {
    let col = marker.column as usize;
    let row = marker.row as usize;
    let col_prefix = sheet.column_widths.len().min(col);
    let extra_cols = col.saturating_sub(sheet.column_widths.len());
    let row_prefix = sheet.row_heights.len().min(row);
    let extra_rows = row.saturating_sub(sheet.row_heights.len());
    (
        sheet.column_widths[..col_prefix].iter().sum::<f32>()
            + (extra_cols as f32) * default_column_width
            + marker.column_offset,
        sheet.row_heights[..row_prefix].iter().sum::<f32>()
            + (extra_rows as f32) * default_row_height
            + marker.row_offset,
    )
}

fn can_cover_extent(
    current_len: usize,
    current_sum: f32,
    min_len: usize,
    target_end: f32,
    default_size: f32,
    max_len: usize,
) -> bool {
    let mut len = current_len.max(min_len);
    if len > max_len {
        return false;
    }
    let mut total = current_sum + ((len - current_len) as f32) * default_size;
    let step = default_size.max(1.0);
    while total < target_end && len < max_len {
        len += 1;
        total += step;
    }
    total >= target_end
}

fn apply_extent(
    sheet: &mut Sheet,
    plan: ExtentPlan,
    default_row_height: f32,
    default_column_width: f32,
) {
    match plan {
        ExtentPlan::Markers { rows, columns } => {
            if sheet.row_heights.len() < rows {
                sheet.row_heights.resize(rows, default_row_height);
            }
            if sheet.column_widths.len() < columns {
                sheet.column_widths.resize(columns, default_column_width);
            }
        }
        ExtentPlan::Extent {
            min_rows,
            min_columns,
            right,
            bottom,
        } => {
            if sheet.row_heights.len() < min_rows {
                sheet.row_heights.resize(min_rows, default_row_height);
            }
            if sheet.column_widths.len() < min_columns {
                sheet
                    .column_widths
                    .resize(min_columns, default_column_width);
            }
            let row_height = default_row_height.max(1.0);
            let column_width = default_column_width.max(1.0);
            let mut current_bottom = sheet.row_heights.iter().sum::<f32>();
            while current_bottom < bottom && sheet.row_heights.len() < MAX_ROWS as usize {
                sheet.row_heights.push(row_height);
                current_bottom += row_height;
            }
            let mut current_right = sheet.column_widths.iter().sum::<f32>();
            while current_right < right && sheet.column_widths.len() < MAX_COLUMNS as usize {
                sheet.column_widths.push(column_width);
                current_right += column_width;
            }
        }
    }
}
