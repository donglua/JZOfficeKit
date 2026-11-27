use super::{coordinates, unsigned, Workbook, MAX_COLUMNS, MAX_ROWS};
use crate::model::{Document, Element, ElementType, Sheet};
use crate::pptx::geometry::{points, Bounds};
use crate::pptx::transform::{flipped, Transform};
use crate::xml::Node;
use crate::{Error, Package, Result};

const MAX_CHARTS_PER_SHEET: usize = 128;
const MAX_COORDINATE: f32 = 100_000.0;
/// 恶意深层 `<xdr:grpSp>` 递归上限，避免栈溢出。
const MAX_GROUP_DEPTH: usize = 16;
/// Excel 形状文本默认字号（point）。
const DEFAULT_SHAPE_FONT_SIZE: f32 = 11.0;

const WARN_UNSUPPORTED: &str =
    "Worksheet drawings other than supported charts, pictures, and shapes are not rendered";
const WARN_INVALID_ANCHOR: &str = "XLSX drawings with invalid or empty anchors were omitted.";
const WARN_EXCESSIVE: &str = "XLSX elements with excessive transformed bounds were omitted.";
const WARN_MISSING_PICTURE: &str = "XLSX linked or missing pictures were omitted.";
const WARN_INVALID_CROP: &str = "XLSX pictures with invalid crop bounds use the full image.";
const WARN_PICTURE_MASK: &str = "XLSX picture masks use rectangular bounds.";
const WARN_GROUP_FLIP: &str = "XLSX group flips are not supported.";
const WARN_GROUP_TRANSFORM: &str = "XLSX groups with invalid or excessive transforms were omitted.";

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

/// 解析工作表 DrawingML：内嵌图表、浮动图片、形状、连接线与组合图形统一编译为 `Sheet.elements`。
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
        let Some(payload) = anchor_payload(anchor, document) else {
            unsupported = true;
            continue;
        };
        // 隐藏图形直接跳过：不计预算、不扩展工作表行列
        if is_hidden(payload) {
            continue;
        }
        // 只读推算定位与扩展计划；确认负载有效并完成编译后再提交行列扩展，
        // 避免无效或被丢弃的图形撑大工作表。
        let Some((frame, extent_plan)) =
            resolve_frame(anchor, sheet, defaults.row_height, defaults.column_width)?
        else {
            document.warn(WARN_INVALID_ANCHOR);
            continue;
        };
        let mut elements = Vec::new();
        match payload.name.as_str() {
            "graphicFrame" => {
                let Some(chart) = payload.descendant("chart") else {
                    unsupported = true;
                    continue;
                };
                chart_count += 1;
                if chart_count > MAX_CHARTS_PER_SHEET {
                    return Err(Error::Limit("XLSX worksheet exceeds 128 charts"));
                }
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
                let Some(mut chart_elements) = crate::chart::render(
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
                for element in &mut chart_elements {
                    // 共享引擎输出图表局部坐标；Sheet 与 PPTX 一样通过 transform 平移到目标坐标系。
                    element.x += frame.x;
                    element.y += frame.y;
                    element.transform = crate::pptx::drawing::element_transform(element);
                }
                elements = chart_elements;
            }
            "pic" => {
                let bounds = shape_bounds(payload, frame);
                let transform = Transform::IDENTITY;
                if let Some(element) = compile_picture(
                    payload,
                    bounds,
                    transform,
                    package,
                    &drawing_relationships,
                    &workbook.theme,
                    document,
                ) {
                    push_element(&mut elements, element, transform, document);
                }
            }
            "sp" | "cxnSp" => {
                let bounds = shape_bounds(payload, frame);
                let transform = Transform::IDENTITY;
                compile_shape(
                    payload,
                    bounds,
                    transform,
                    workbook,
                    document,
                    &mut elements,
                )?;
            }
            "grpSp" => {
                let Some(group_transform) = Transform::IDENTITY.group_in_frame(
                    payload,
                    frame.x,
                    frame.y,
                    frame.width,
                    frame.height,
                ) else {
                    document.warn(WARN_GROUP_TRANSFORM);
                    continue;
                };
                if flipped(payload) {
                    document.warn(WARN_GROUP_FLIP);
                }
                compile_group(
                    payload,
                    group_transform,
                    1,
                    package,
                    &drawing_relationships,
                    workbook,
                    document,
                    &mut elements,
                )?;
            }
            _ => {
                unsupported = true;
                continue;
            }
        }
        // 原子提交：仅在本次锚点确实产出元素时才扣除预算并扩展工作表行列
        if elements.is_empty() {
            continue;
        }
        let command_count = elements
            .iter()
            .flat_map(|element| &element.paths)
            .map(|path| path.commands.len())
            .sum::<usize>();
        workbook.budget.elements(elements.len())?;
        workbook.budget.path_commands(command_count)?;
        apply_extent(
            sheet,
            extent_plan,
            defaults.row_height,
            defaults.column_width,
        );
        sheet.elements.extend(elements);
    }
    if unsupported {
        document.warn(WARN_UNSUPPORTED);
    }
    Ok(())
}

/// 以锚点 `Frame` 为权威外框，仅从 `spPr/a:xfrm` 提取旋转与翻转。
fn shape_bounds(shape: &Node, frame: Frame) -> Bounds {
    let xfrm = shape.child("spPr").and_then(|p| p.child("xfrm"));
    Bounds::from_frame(frame.x, frame.y, frame.width, frame.height, xfrm)
}

/// 校验变换范围并把元素送入结果列表；越界时丢弃并告警。
fn push_element(
    out: &mut Vec<Element>,
    mut element: Element,
    transform: Transform,
    document: &mut Document,
) {
    element.transform = transform.0;
    if !transform.accepts(&element)
        || !crate::pptx::drawing::prepare_element(&mut element, document)
    {
        document.warn(WARN_EXCESSIVE);
        return;
    }
    out.push(element);
}

/// 编译 `<xdr:pic>`：`blipFill` 取图片部件、`srcRect` 裁剪、`spPr` 旋转翻转与可选外框描边。
fn compile_picture(
    shape: &Node,
    bounds: Bounds,
    transform: Transform,
    package: &Package,
    relationships: &std::collections::HashMap<String, String>,
    theme: &crate::pptx::colors::Theme,
    document: &mut Document,
) -> Option<Element> {
    let fill = shape.child("blipFill");
    let blip = fill.and_then(|n| n.child("blip"));
    let path = blip
        .and_then(|n| relationships.get(crate::pptx::relation_id(n, "embed")))
        .filter(|path| package.has(path));
    let Some(path) = path else {
        document.warn(WARN_MISSING_PICTURE);
        return None;
    };
    let image_crop = fill.and_then(|n| n.child("srcRect")).and_then(|crop| {
        crate::pptx::render::parse_image_crop_with_warning(crop, WARN_INVALID_CROP, document)
    });
    if fill.is_some_and(|n| n.child("tile").is_some())
        || blip.is_some_and(|n| !n.children.is_empty())
    {
        document.warn("XLSX picture tiling and effects are not supported.");
    }
    let mut element = bounds.element(ElementType::IMAGE);
    element.image = Some(path.clone());
    element.image_crop = image_crop;
    element.paths = crate::pptx::styles::Styles::picture_mask_paths(
        shape,
        [element.width, element.height],
        document,
        WARN_PICTURE_MASK,
    );
    // 图片外框描边：仅在 `spPr/a:ln` 明确给出可见线型时生效；使用 `line_only`
    // 避免非矩形图片蒙版（例如 cloud）进入几何分支误发 PPTX 告警并丢弃边框。
    if let Some(line) = shape.child("spPr").and_then(|p| p.child("ln")) {
        if line.child("noFill").is_none() {
            let mut styled = bounds.element(ElementType::IMAGE);
            {
                let doc = &mut *document;
                crate::pptx::styles::Styles { doc, theme }.line_only(shape, &mut styled);
            }
            element.stroke = styled.stroke;
            element.stroke_width = styled.stroke_width;
            element.stroke_dash = styled.stroke_dash;
        }
    }
    element.transform = transform.0;
    Some(element)
}

/// 编译 `<xdr:sp>` / `<xdr:cxnSp>`：预设几何、填充、描边、箭头端点与 `txBody` 文本。
fn compile_shape(
    shape: &Node,
    bounds: Bounds,
    transform: Transform,
    workbook: &mut Workbook,
    document: &mut Document,
    out: &mut Vec<Element>,
) -> Result<()> {
    let mut element = bounds.element(ElementType::RECT);
    let supported = {
        let (theme, doc) = (&workbook.theme, &mut *document);
        crate::pptx::styles::Styles { doc, theme }.drawing(&[shape], &mut element)
    };
    let body = shape.child("txBody");
    // 与 PPTX 一致：无可见视觉且仅有文本时不重复输出空形状
    if supported
        && (element.fill != 0
            || element.fill_gradient.is_some()
            || element.stroke != 0
            || body.is_none())
    {
        push_element(out, element, transform, document);
    }
    if let Some(body) = body {
        let Some(mut text) = bounds.text_element(&[shape], document) else {
            document.warn("XLSX text with empty or excessive bounds was omitted.");
            return Ok(());
        };
        let budget = &mut workbook.budget;
        let theme = &workbook.theme;
        text.paragraphs = crate::pptx::text::read_drawing_paragraphs(
            body,
            shape,
            theme,
            DEFAULT_SHAPE_FONT_SIZE,
            document,
            &mut |bytes| budget.text_bytes(bytes),
        )?;
        if !text.paragraphs.is_empty() {
            push_element(out, text, transform, document);
        }
    }
    Ok(())
}

/// 递归展开 `<xdr:grpSp>`：子群组复合变换，子图形按各自 `Bounds` 换算到数据区坐标。
#[allow(clippy::too_many_arguments)]
fn compile_group(
    group: &Node,
    group_transform: Transform,
    depth: usize,
    package: &Package,
    relationships: &std::collections::HashMap<String, String>,
    workbook: &mut Workbook,
    document: &mut Document,
    out: &mut Vec<Element>,
) -> Result<()> {
    if depth > MAX_GROUP_DEPTH {
        return Err(Error::Limit("XLSX drawing group nesting exceeds 16 levels"));
    }
    for child in &group.children {
        match child.name.as_str() {
            "nvGrpSpPr" | "grpSpPr" | "extLst" | "clientData" => continue,
            "AlternateContent" => {
                if let Some((branch, lossless)) = select_alternate(child) {
                    if !lossless {
                        document.warn(
                            "XLSX alternate content uses one saved representation; unsupported features may be lost.",
                        );
                    }
                    // AlternateContent 只是同一图形的选择分支，不增加 grpSp 嵌套深度。
                    compile_group(
                        branch,
                        group_transform,
                        depth,
                        package,
                        relationships,
                        workbook,
                        document,
                        out,
                    )?;
                } else {
                    document.warn("XLSX alternate content is omitted.");
                }
                continue;
            }
            _ => (),
        }
        if is_hidden(child) {
            continue;
        }
        match child.name.as_str() {
            "grpSp" => {
                let Some(transform) = group_transform.group(child) else {
                    document.warn(WARN_GROUP_TRANSFORM);
                    continue;
                };
                if flipped(child) {
                    document.warn(WARN_GROUP_FLIP);
                }
                compile_group(
                    child,
                    transform,
                    depth + 1,
                    package,
                    relationships,
                    workbook,
                    document,
                    out,
                )?;
            }
            "pic" | "sp" | "cxnSp" => {
                let Some(raw) = Bounds::parse(&[child]) else {
                    document.warn(WARN_INVALID_ANCHOR);
                    continue;
                };
                let Some((bounds, transform)) = raw.in_slide_units(group_transform) else {
                    document.warn(WARN_EXCESSIVE);
                    continue;
                };
                if child.name == "pic" {
                    if let Some(element) = compile_picture(
                        child,
                        bounds,
                        transform,
                        package,
                        relationships,
                        &workbook.theme,
                        document,
                    ) {
                        push_element(out, element, transform, document);
                    }
                } else {
                    compile_shape(child, bounds, transform, workbook, document, out)?;
                }
            }
            _ => document.warn(WARN_UNSUPPORTED),
        }
    }
    Ok(())
}

/// 定位锚点下的实际图形负载节点（跳过 `from` / `to` / `pos` / `ext` / `clientData`）。
fn anchor_payload<'a>(anchor: &'a Node, document: &mut Document) -> Option<&'a Node> {
    for node in &anchor.children {
        match node.name.as_str() {
            "graphicFrame" | "pic" | "sp" | "cxnSp" | "grpSp" => return Some(node),
            "AlternateContent" => {
                if let Some((branch, lossless)) = select_alternate(node) {
                    if !lossless {
                        document.warn(
                            "XLSX alternate content uses one saved representation; unsupported features may be lost.",
                        );
                    }
                    if let Some(payload) = anchor_payload(branch, document) {
                        return Some(payload);
                    }
                } else {
                    document.warn("XLSX alternate content is omitted.");
                }
            }
            _ => (),
        }
    }
    None
}

/// 检查 `cNvPr@hidden`：`nvPicPr` / `nvSpPr` / `nvCxnSpPr` / `nvGrpSpPr` 下统一生效。
fn is_hidden(shape: &Node) -> bool {
    shape
        .children
        .iter()
        .find(|n| n.name.starts_with("nv"))
        .and_then(|n| n.child("cNvPr"))
        .is_some_and(|n| crate::pptx::flag(n.attr("hidden"), false))
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
