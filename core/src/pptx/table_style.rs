//! PPTX `ppt/tableStyles.xml` 表样式级联。
//!
//! PowerPoint 把表格外观拆成两层：`<a:tbl>` 内的 `<a:tblPr>` 只记录开关与样式引用，
//! 真正的填充、边框与文本样式存放在包级 `ppt/tableStyles.xml` 的 `<a:tblStyleLst>` 中。
//! 本模块按 OOXML 规定的优先级把 13 个条件样式块、`<a:tblBg>` 与 `<a:tcPr>` 归并成
//! 每格的填充、四边边框与文本基线，供 `table.rs` 一次性写入文档模型，从而让三端渲染层
//! 只需要 `O(1)` 查表而无需理解表样式语义。

use super::colors::Theme;
use super::geometry::points;
use super::{flag, Budget};
use crate::model::{CellBorder, Document};
use crate::xml::Node;

/// 四边下标，顺序与 `cell_borders` / `cell_paddings` 一致：`[top, right, bottom, left]`。
pub(super) const EDGE_TOP: usize = 0;
pub(super) const EDGE_RIGHT: usize = 1;
pub(super) const EDGE_BOTTOM: usize = 2;
pub(super) const EDGE_LEFT: usize = 3;

/// 单元格单边边框的三种状态。
///
/// `Unset` 表示该样式块没有声明这条边，级联时必须保留已有结果；`Hidden` 表示显式
/// `<a:noFill/>` 或 `w="0"`，需要盖掉下层样式；`Line` 为可见实线。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Edge {
    Unset,
    Hidden,
    Line(CellBorder),
}

/// 单个单元格由表样式继承的文本基线。
///
/// 字段均为 `Option`，未声明时保持 `None`，交给 `table.rs` 与单元格自身 `rPr` 决定。
#[derive(Debug, Clone, Default)]
pub(super) struct TextDefaults {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub color: Option<u32>,
    /// 主题字体引用 `fontRef/@idx`，取值 `"major"` / `"minor"`。
    pub font_key: Option<String>,
}

/// 整张表的级联结果，三个矩阵均为 `rows × cols`。
pub(super) struct ResolvedTableStyles {
    pub fills: Vec<Vec<Option<u32>>>,
    pub borders: Vec<Vec<[Edge; 4]>>,
    pub texts: Vec<Vec<TextDefaults>>,
    /// 是否至少有一条边被级联写入（存在可见线或显式隐藏）。
    ///
    /// 仅"样式里声明了 `<a:tcBdr>`"不算：若该节未命中任何单元格（例如样式定义了 `lastRow`
    /// 但 `<a:tblPr>` 未开启 `lastRow`），全表实际没有边框，必须让平台层沿用默认网格。
    pub has_any_border_rule: bool,
}

impl ResolvedTableStyles {
    fn empty(rows: usize, cols: usize) -> Self {
        Self {
            fills: vec![vec![None; cols]; rows],
            borders: vec![vec![[Edge::Unset; 4]; cols]; rows],
            texts: vec![vec![TextDefaults::default(); cols]; rows],
            has_any_border_rule: false,
        }
    }
}

/// `<a:tblPr>` 的六个布尔开关与样式引用。
struct TableSwitches {
    first_row: bool,
    last_row: bool,
    first_col: bool,
    last_col: bool,
    band_row: bool,
    band_col: bool,
    style_id: Option<String>,
}

impl TableSwitches {
    fn read(table: &Node) -> Self {
        let Some(properties) = table.child("tblPr") else {
            return Self {
                first_row: false,
                last_row: false,
                first_col: false,
                last_col: false,
                band_row: false,
                band_col: false,
                style_id: None,
            };
        };
        Self {
            first_row: flag(properties.attr("firstRow"), false),
            last_row: flag(properties.attr("lastRow"), false),
            first_col: flag(properties.attr("firstCol"), false),
            last_col: flag(properties.attr("lastCol"), false),
            band_row: flag(properties.attr("bandRow"), false),
            band_col: flag(properties.attr("bandCol"), false),
            style_id: properties
                .child("tableStyleId")
                .map(|n| n.text.trim().to_owned())
                .filter(|s| !s.is_empty()),
        }
    }
}

/// 在 `<a:tblStyleLst>` 中按 `styleId` 查找表样式定义。
///
/// `styleId` 为花括号包裹的 GUID，按大小写不敏感匹配以容忍非规范生成器。
fn find_style<'a>(styles: &'a Node, style_id: &str) -> Option<&'a Node> {
    let list = styles.child("tblStyleLst").or_else(|| {
        if styles.name == "tblStyleLst" {
            Some(styles)
        } else {
            None
        }
    })?;
    list.named("tblStyle")
        .find(|n| n.attr("styleId").eq_ignore_ascii_case(style_id))
}

/// 计算某个样式节对 `(r, c)` 的命中判定，返回 `(命中, 该节的行集, 列集)` 的谓词。
///
/// 只关心"是否命中"，但 `<a:tcBdr>` 的 insideH/insideV 与外框边需要知道命中区域边界，
/// 故统一用 `matches(r, c)` 闭包判断相邻格是否同属该节。
fn section_matches(
    name: &str,
    switches: &TableSwitches,
    rows: usize,
    cols: usize,
) -> impl Fn(usize, usize) -> bool {
    // 行带子区起止：启用 firstRow 时数据区从第 1 行开始；启用 lastRow 时到倒数第 1 行结束
    let band_row_start = usize::from(switches.first_row);
    let band_row_end = if switches.last_row && rows > band_row_start {
        rows - 1
    } else {
        rows
    };
    let band_col_start = usize::from(switches.first_col);
    let band_col_end = if switches.last_col && cols > band_col_start {
        cols - 1
    } else {
        cols
    };
    let first_row = switches.first_row;
    let last_row = switches.last_row;
    let first_col = switches.first_col;
    let last_col = switches.last_col;
    let band_row = switches.band_row;
    let band_col = switches.band_col;
    let section = name.to_owned();
    move |r: usize, c: usize| match section.as_str() {
        "wholeTbl" => true,
        "band1V" => {
            band_col && c >= band_col_start && c < band_col_end && (c - band_col_start) % 2 == 0
        }
        "band2V" => {
            band_col && c >= band_col_start && c < band_col_end && (c - band_col_start) % 2 == 1
        }
        "band1H" => {
            band_row && r >= band_row_start && r < band_row_end && (r - band_row_start) % 2 == 0
        }
        "band2H" => {
            band_row && r >= band_row_start && r < band_row_end && (r - band_row_start) % 2 == 1
        }
        "firstCol" => first_col && c == 0,
        "lastCol" => last_col && cols > 0 && c + 1 == cols,
        "firstRow" => first_row && r == 0,
        "lastRow" => last_row && rows > 0 && r + 1 == rows,
        "nwCell" => first_row && first_col && r == 0 && c == 0,
        "neCell" => first_row && last_col && r == 0 && cols > 0 && c + 1 == cols,
        "swCell" => last_row && first_col && rows > 0 && r + 1 == rows && c == 0,
        "seCell" => last_row && last_col && rows > 0 && r + 1 == rows && cols > 0 && c + 1 == cols,
        _ => false,
    }
}

/// 级联顺序（低优先 → 高优先），同 OOXML `tblStyle` 子元素约定。
const SECTIONS: [&str; 13] = [
    "wholeTbl", "band1V", "band2V", "band1H", "band2H", "firstCol", "lastCol", "firstRow",
    "lastRow", "nwCell", "neCell", "swCell", "seCell",
];

/// 解析 `<a:tbl>` 的完整表样式级联结果。
///
/// `table_styles` 为 `ppt/tableStyles.xml` 的根节点；缺失或 `styleId` 未命中时只保留
/// `<a:tblBg>` 并发出一次提示，随后仍按无表样式继续解析。
pub(super) fn resolve_table_style(
    table: &Node,
    rows: usize,
    cols: usize,
    table_styles: Option<&Node>,
    theme: &Theme,
    budget: &mut Budget,
    doc: &mut Document,
) -> ResolvedTableStyles {
    let mut resolved = ResolvedTableStyles::empty(rows, cols);
    if rows == 0 || cols == 0 {
        return resolved;
    }
    let switches = TableSwitches::read(table);
    let properties = table.child("tblPr");

    // 1. <a:tblBg> 只贡献填充，且优先级低于所有样式节
    if let Some(background) = properties.and_then(|p| p.child("tblBg")) {
        if let Some(fill) = parse_fill(background, theme, doc) {
            for row in &mut resolved.fills {
                for cell in row.iter_mut() {
                    *cell = Some(fill);
                }
            }
        }
    }

    // 2. 按 styleId 查找外部表样式定义
    let style = switches.style_id.as_deref().and_then(|style_id| {
        let Some(part) = table_styles else {
            doc.warn("PPTX table style references a missing ppt/tableStyles.xml.");
            return None;
        };
        match find_style(part, style_id) {
            Some(style) => Some(style),
            None => {
                doc.warn("PPTX table style is not defined in ppt/tableStyles.xml.");
                None
            }
        }
    });

    // 3. 逐节级联填充、边框与文本样式
    if let Some(style) = style {
        for name in SECTIONS {
            let Some(section) = style.child(name) else {
                continue;
            };
            let matches = section_matches(name, &switches, rows, cols);
            if budget.object().is_err() {
                doc.warn("PPTX table style cascade exceeded its element budget.");
                break;
            }
            apply_section(&mut resolved, section, &matches, rows, cols, theme, doc);
        }
    }

    resolved
}

/// 把一个样式节叠加到级联结果上。
fn apply_section(
    resolved: &mut ResolvedTableStyles,
    section: &Node,
    matches: &impl Fn(usize, usize) -> bool,
    rows: usize,
    cols: usize,
    theme: &Theme,
    doc: &mut Document,
) {
    let fill = section
        .child("tcStyle")
        .and_then(|style| parse_fill(style, theme, doc));
    let border = section
        .child("tcStyle")
        .and_then(|style| style.child("tcBdr"));
    let text = section
        .child("tcTxStyle")
        .map(|style| parse_text_style(style, theme, doc))
        .unwrap_or_default();

    for r in 0..rows {
        for c in 0..cols {
            if !matches(r, c) {
                continue;
            }
            if let Some(fill) = fill {
                resolved.fills[r][c] = Some(fill);
            }
            // 文本样式按字段逐项覆盖，未声明的字段保留下层结果
            let target = &mut resolved.texts[r][c];
            if text.bold.is_some() {
                target.bold = text.bold;
            }
            if text.italic.is_some() {
                target.italic = text.italic;
            }
            if text.color.is_some() {
                target.color = text.color;
            }
            if text.font_key.is_some() {
                target.font_key = text.font_key.clone();
            }
            if let Some(border) = border {
                apply_borders(resolved, border, r, c, matches, rows, cols, theme, doc);
            }
        }
    }
}

/// 把 `<a:tcBdr>` 的四条外框边与两条内部边写到命中区域的正确位置。
///
/// 判定规则：命中区域的外缘写 `top` / `bottom` / `left` / `right`；
/// 区域内部相邻格之间写 `insideH` / `insideV`。
#[allow(clippy::too_many_arguments)]
fn apply_borders(
    resolved: &mut ResolvedTableStyles,
    border: &Node,
    r: usize,
    c: usize,
    matches: &impl Fn(usize, usize) -> bool,
    rows: usize,
    cols: usize,
    theme: &Theme,
    doc: &mut Document,
) {
    // 上边：本行是区域首行时取外框 top，否则取 insideH
    let top_edge = if r == 0 || !matches(r - 1, c) {
        border.child("top")
    } else {
        border.child("insideH")
    };
    // 下边：本行是区域末行时取外框 bottom，否则取 insideH
    let bottom_edge = if r + 1 == rows || !matches(r + 1, c) {
        border.child("bottom")
    } else {
        border.child("insideH")
    };
    let left_edge = if c == 0 || !matches(r, c - 1) {
        border.child("left")
    } else {
        border.child("insideV")
    };
    let right_edge = if c + 1 == cols || !matches(r, c + 1) {
        border.child("right")
    } else {
        border.child("insideV")
    };

    for (index, node) in [
        (EDGE_TOP, top_edge),
        (EDGE_RIGHT, right_edge),
        (EDGE_BOTTOM, bottom_edge),
        (EDGE_LEFT, left_edge),
    ] {
        let Some(node) = node else { continue };
        // <a:top>/<a:right>/... 是容器，真正的线型定义在其 <a:ln> 或 <a:lnRef> 子元素里
        let Some(line) = node.child("ln").or_else(|| node.child("lnRef")) else {
            continue;
        };
        let edge = if line.name == "lnRef" {
            parse_line_ref(line, theme, doc)
        } else {
            parse_line(line, theme, doc)
        };
        // Unset 不覆盖已有级联结果；Hidden 与 Line 才生效
        if edge != Edge::Unset {
            resolved.borders[r][c][index] = edge;
            resolved.has_any_border_rule = true;
        }
    }
}

/// 解析样式节里的填充：`<a:tcStyle>` 内部可能直接是 `noFill` / `solidFill`，
/// 也可能包一层 `<a:fill>` 再放填充子树。
fn parse_fill(container: &Node, theme: &Theme, doc: &mut Document) -> Option<u32> {
    // 优先处理 <a:fill>/<a:fillRef> 包装形态
    if let Some(fill) = container.child("fill") {
        return parse_fill(fill, theme, doc);
    }
    if let Some(reference) = container.child("fillRef") {
        let value = theme.fill_reference(reference, theme.text_color(), doc);
        return Some(value);
    }
    if container.child("noFill").is_some() {
        return Some(0);
    }
    if container.child("solidFill").is_some() {
        return Some(theme.fill(container, 0, doc));
    }
    if container.children.iter().any(|n| {
        matches!(
            n.name.as_str(),
            "gradFill" | "pattFill" | "blipFill" | "grpFill"
        )
    }) {
        // 渐变等填充统一按纯色兜底，保持与 fill_reference 一致的降级策略
        return Some(theme.fill(container, 0, doc));
    }
    None
}

/// 解析 `<a:tcTxStyle>` 的粗体、斜体、字色与主题字体引用。
fn parse_text_style(style: &Node, theme: &Theme, doc: &mut Document) -> TextDefaults {
    let mut result = TextDefaults {
        bold: parse_on_off(style.attr("b")),
        italic: parse_on_off(style.attr("i")),
        ..TextDefaults::default()
    };
    // 字体色优先取 tcTxStyle 的直接颜色子节点，其次取 fontRef 内颜色
    let direct_color = style
        .children
        .iter()
        .any(|n| matches!(n.name.as_str(), "srgbClr" | "schemeClr" | "sysClr"));
    if direct_color {
        result.color = Some(theme.color(style, theme.text_color(), doc));
    }
    if let Some(reference) = style.child("fontRef") {
        if !direct_color {
            result.color = Some(theme.color(reference, theme.text_color(), doc));
        }
        // fontRef/@idx 用 "major" / "minor" 表示主题字体分组
        let key = reference.attr("idx");
        if matches!(key, "major" | "minor") {
            result.font_key = Some(key.to_owned());
        }
    }
    result
}

/// 解析 OOXML 的 on/off 属性值。
fn parse_on_off(value: &str) -> Option<bool> {
    match value {
        "on" | "1" | "true" => Some(true),
        "off" | "0" | "false" => Some(false),
        _ => None,
    }
}

/// 解析单条 `<a:ln>` 边框。
///
/// - `<a:noFill/>` 或 `w="0"` 视为显式隐藏；
/// - 有线宽但无显式填充时取主题文字色（DrawingML 缺省行为）；
/// - 线宽钳位 `[0.25, 36.0]`，未声明时默认 `1.0pt`；
/// - 非实线 `prstDash` 降级为实线并提示一次。
pub(super) fn parse_line(ln: &Node, theme: &Theme, doc: &mut Document) -> Edge {
    if ln.child("noFill").is_some() || points(ln.attr("w")) == Some(0.0) {
        return Edge::Hidden;
    }
    let color = ln
        .child("solidFill")
        .map(|fill| theme.color(fill, theme.text_color(), doc))
        .unwrap_or_else(|| theme.text_color());
    if let Some(dash) = ln.child("prstDash") {
        let value = dash.attr("val");
        if !value.is_empty() && value != "solid" {
            doc.warn("PPTX dashed table cell borders are rendered solid.");
        }
    }
    let width = points(ln.attr("w")).unwrap_or(1.0).clamp(0.25, 36.0);
    Edge::Line(CellBorder { color, width })
}

/// 解析 `<a:lnRef>`：`idx="0"` 为隐藏，其余按主题 `lnStyleLst` 取线宽基线，
/// 颜色以引用内颜色替换主题占位色。
fn parse_line_ref(reference: &Node, theme: &Theme, doc: &mut Document) -> Edge {
    let index = reference.attr("idx").parse::<usize>().unwrap_or(0);
    if index == 0 {
        return Edge::Hidden;
    }
    let width = theme
        .format_list("lnStyleLst")
        .and_then(|list| list.children.get(index - 1))
        .and_then(|style| points(style.attr("w")))
        .unwrap_or(1.0)
        .clamp(0.25, 36.0);
    let color = theme.color(reference, theme.text_color(), doc);
    Edge::Line(CellBorder { color, width })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Document, Kind};
    use crate::xml;

    /// 构造一个带样式引用的最小 `<a:tbl>`：`switches` 是 `<a:tblPr>` 的属性串。
    /// `switches` 为 `<a:tblPr>` 的开关属性，`style_id` 以子元素 `<a:tableStyleId>` 形式写入
    /// （OOXML 中它是 `<a:tblPr>` 的子元素而非属性），`background` 为可选的 `<a:tblBg>` 子树。
    fn build_table(
        switches: &str,
        style_id: &str,
        rows: usize,
        cols: usize,
    ) -> crate::Result<Node> {
        let grid = (0..cols)
            .map(|_| r#"<a:gridCol w="1000000"/>"#)
            .collect::<String>();
        let body = (0..rows)
            .map(|_| {
                let cells = (0..cols)
                    .map(|_| "<a:tc><a:tcPr/></a:tc>")
                    .collect::<String>();
                format!("<a:tr>{cells}</a:tr>")
            })
            .collect::<String>();
        let style = if style_id.is_empty() {
            String::new()
        } else {
            format!("<a:tableStyleId>{style_id}</a:tableStyleId>")
        };
        xml::parse(
            format!(
                r#"<a:tbl xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
                    <a:tblPr {switches}>{style}</a:tblPr><a:tblGrid>{grid}</a:tblGrid>{body}
                </a:tbl>"#
            )
            .into_bytes(),
        )
    }

    fn theme() -> crate::Result<Theme> {
        let node = xml::parse(
            br#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
                <a:themeElements>
                    <a:clrScheme name="C">
                        <a:dk1><a:srgbClr val="000000"/></a:dk1>
                        <a:lt1><a:srgbClr val="FFFFFF"/></a:lt1>
                        <a:accent1><a:srgbClr val="2A9D8F"/></a:accent1>
                        <a:accent2><a:srgbClr val="E9C46A"/></a:accent2>
                    </a:clrScheme>
                    <a:fmtScheme name="F"><a:fillStyleLst/><a:lnStyleLst/></a:fmtScheme>
                </a:themeElements>
            </a:theme>"#
                .to_vec(),
        )?;
        let mut doc = Document::new(Kind::PPTX);
        Ok(Theme::from_node(Some(&node), &mut doc))
    }

    fn resolve(
        table_xml: &Node,
        styles_xml: Option<&str>,
        rows: usize,
        cols: usize,
    ) -> crate::Result<ResolvedTableStyles> {
        let styles = styles_xml
            .map(|text| xml::parse(text.as_bytes().to_vec()))
            .transpose()?;
        let theme = theme()?;
        let mut budget = Budget::default();
        let mut doc = Document::new(Kind::PPTX);
        Ok(resolve_table_style(
            table_xml,
            rows,
            cols,
            styles.as_ref(),
            &theme,
            &mut budget,
            &mut doc,
        ))
    }

    /// 覆盖 wholeTbl + band1H/band2H + firstRow + firstCol + nwCell 的级联优先级与 tcTxStyle。
    #[test]
    fn cascade_applies_banding_header_and_corner_styles() -> crate::Result<()> {
        let table_xml = build_table(
            r#"firstRow="1" firstCol="1" bandRow="1""#,
            "{STYLE-1}",
            4,
            3,
        )?;
        let styles = r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <a:tblStyle styleId="{STYLE-1}">
                <a:wholeTbl><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="111111"/></a:solidFill></a:fill></a:tcStyle></a:wholeTbl>
                <a:band1H><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="222222"/></a:solidFill></a:fill></a:tcStyle></a:band1H>
                <a:band2H><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="333333"/></a:solidFill></a:fill></a:tcStyle></a:band2H>
                <a:firstCol><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="444444"/></a:solidFill></a:fill></a:tcStyle></a:firstCol>
                <a:firstRow>
                    <a:tcStyle><a:fill><a:solidFill><a:srgbClr val="555555"/></a:solidFill></a:fill></a:tcStyle>
                    <a:tcTxStyle b="on"><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></a:tcTxStyle>
                </a:firstRow>
                <a:nwCell><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="666666"/></a:solidFill></a:fill></a:tcStyle></a:nwCell>
            </a:tblStyle>
        </a:tblStyleLst>"#;
        let resolved = resolve(&table_xml, Some(styles), 4, 3)?;

        // nwCell 最高优先
        assert_eq!(resolved.fills[0][0], Some(0xff66_6666));
        // 表头其余格取 firstRow
        assert_eq!(resolved.fills[0][1], Some(0xff55_5555));
        assert_eq!(resolved.fills[0][2], Some(0xff55_5555));
        // 第 1 列非表头格取 firstCol（firstCol 优先于 band2H）
        assert_eq!(resolved.fills[1][0], Some(0xff44_4444));
        // 数据区行带从第 1 行开始：第 1 行命中 band1H
        assert_eq!(resolved.fills[1][1], Some(0xff22_2222));
        // 第 2 行命中 band2H
        assert_eq!(resolved.fills[2][1], Some(0xff33_3333));
        // 第 3 行再次命中 band1H
        assert_eq!(resolved.fills[3][1], Some(0xff22_2222));

        // 表头 tcTxStyle：粗体 + 主题浅色字
        assert_eq!(resolved.texts[0][1].bold, Some(true));
        assert_eq!(resolved.texts[0][1].color, Some(0xffff_ffff));
        assert_eq!(resolved.texts[0][1].font_key.as_deref(), Some("minor"));
        // 数据区未声明 tcTxStyle
        assert_eq!(resolved.texts[1][1].bold, None);
        Ok(())
    }

    /// 覆盖 lastRow / lastCol / bandCol / neCell / swCell / seCell 的级联与列带奇偶判定。
    #[test]
    fn cascade_applies_last_row_column_banding_and_corners() -> crate::Result<()> {
        let table_xml = build_table(
            r#"firstRow="1" lastRow="1" firstCol="1" lastCol="1" bandRow="1" bandCol="1""#,
            "{STYLE-3}",
            4,
            4,
        )?;
        let styles = r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <a:tblStyle styleId="{STYLE-3}">
                <a:wholeTbl><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="010101"/></a:solidFill></a:fill></a:tcStyle></a:wholeTbl>
                <a:band1V><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="020202"/></a:solidFill></a:fill></a:tcStyle></a:band1V>
                <a:band2V><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="030303"/></a:solidFill></a:fill></a:tcStyle></a:band2V>
                <a:band1H><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="040404"/></a:solidFill></a:fill></a:tcStyle></a:band1H>
                <a:band2H><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="050505"/></a:solidFill></a:fill></a:tcStyle></a:band2H>
                <a:firstCol><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="060606"/></a:solidFill></a:fill></a:tcStyle></a:firstCol>
                <a:lastCol><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="070707"/></a:solidFill></a:fill></a:tcStyle></a:lastCol>
                <a:firstRow><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="080808"/></a:solidFill></a:fill></a:tcStyle></a:firstRow>
                <a:lastRow><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="090909"/></a:solidFill></a:fill></a:tcStyle></a:lastRow>
                <a:neCell><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="0A0A0A"/></a:solidFill></a:fill></a:tcStyle></a:neCell>
                <a:swCell><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="0B0B0B"/></a:solidFill></a:fill></a:tcStyle></a:swCell>
                <a:seCell><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="0C0C0C"/></a:solidFill></a:fill></a:tcStyle></a:seCell>
            </a:tblStyle>
        </a:tblStyleLst>"#;
        let resolved = resolve(&table_xml, Some(styles), 4, 4)?;

        // 四角：nwCell 未定义时由 firstRow 与 firstCol 叠加决定，neCell / swCell / seCell 最高优先
        assert_eq!(resolved.fills[0][3], Some(0xff0a_0a0a), "neCell fill");
        assert_eq!(resolved.fills[3][0], Some(0xff0b_0b0b), "swCell fill");
        assert_eq!(resolved.fills[3][3], Some(0xff0c_0c0c), "seCell fill");
        // lastRow 覆盖数据区行带
        assert_eq!(
            resolved.fills[3][1],
            Some(0xff09_0909),
            "lastRow fill beats band2H"
        );
        // lastCol 覆盖数据区列带
        assert_eq!(
            resolved.fills[1][3],
            Some(0xff07_0707),
            "lastCol fill beats band2V"
        );
        // 数据区起止：firstRow/firstCol 后第 1 行命中 band1H，第 2 行命中 band2H
        assert_eq!(
            resolved.fills[1][1],
            Some(0xff04_0404),
            "band1H on first data row"
        );
        assert_eq!(
            resolved.fills[2][1],
            Some(0xff05_0505),
            "band2H on second data row"
        );
        // 列带：数据区第 1 列命中 band1V（被 band1H 覆盖），第 2 列命中 band2V 后再被 band2H 覆盖
        assert_eq!(
            resolved.fills[1][2],
            Some(0xff04_0404),
            "band1H wins over band2V"
        );
        Ok(())
    }

    /// `<a:tcBdr>` 的外框边只落在命中区域外缘，insideH / insideV 只落在区域内部。
    #[test]
    fn tc_bdr_edges_land_on_region_boundaries() -> crate::Result<()> {
        let table_xml = build_table(r#"firstRow="1""#, "{STYLE-2}", 3, 3)?;
        let styles = r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <a:tblStyle styleId="{STYLE-2}">
                <a:firstRow><a:tcStyle><a:tcBdr>
                    <a:top><a:ln w="25400"><a:solidFill><a:srgbClr val="AA0000"/></a:solidFill></a:ln></a:top>
                    <a:bottom><a:ln w="12700"><a:solidFill><a:srgbClr val="00AA00"/></a:solidFill></a:ln></a:bottom>
                    <a:left><a:ln w="12700"><a:solidFill><a:srgbClr val="0000AA"/></a:solidFill></a:ln></a:left>
                    <a:right><a:ln w="12700"><a:solidFill><a:srgbClr val="0000AA"/></a:solidFill></a:ln></a:right>
                    <a:insideV><a:ln w="6350"><a:solidFill><a:srgbClr val="AAAA00"/></a:solidFill></a:ln></a:insideV>
                </a:tcBdr></a:tcStyle></a:firstRow>
            </a:tblStyle>
        </a:tblStyleLst>"#;
        let resolved = resolve(&table_xml, Some(styles), 3, 3)?;

        // 首行首格：外框 top / left，内部右邻取 insideV
        assert!(
            matches!(resolved.borders[0][0][EDGE_TOP], Edge::Line(b) if b.color == 0xffaa_0000)
        );
        assert!(
            matches!(resolved.borders[0][0][EDGE_LEFT], Edge::Line(b) if b.color == 0xff00_00aa)
        );
        assert!(
            matches!(resolved.borders[0][0][EDGE_RIGHT], Edge::Line(b) if b.color == 0xffaa_aa00)
        );
        // 首行中间格：左右都是 insideV
        assert!(
            matches!(resolved.borders[0][1][EDGE_LEFT], Edge::Line(b) if b.color == 0xffaa_aa00)
        );
        assert!(
            matches!(resolved.borders[0][1][EDGE_RIGHT], Edge::Line(b) if b.color == 0xffaa_aa00)
        );
        // 首行末格：外框 right，下边仍是外框 bottom（因为下方不命中区域）
        assert!(
            matches!(resolved.borders[0][2][EDGE_RIGHT], Edge::Line(b) if b.color == 0xff00_00aa)
        );
        assert!(
            matches!(resolved.borders[0][2][EDGE_BOTTOM], Edge::Line(b) if b.color == 0xff00_aa00)
        );
        // 数据区完全未命中，四边保持 Unset
        assert_eq!(resolved.borders[1][0], [Edge::Unset; 4]);
        assert!(resolved.has_any_border_rule);
        Ok(())
    }

    /// `<a:ln>` 的隐藏与降级规则：noFill / w=0 为 Hidden，无色默认主色，虚线触发提示。
    #[test]
    fn line_parsing_handles_hidden_defaults_and_dash_downgrade() -> crate::Result<()> {
        let theme = theme()?;
        let mut doc = Document::new(Kind::PPTX);

        // noFill -> Hidden
        let no_fill = xml::parse(
            br#"<a:ln xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:noFill/></a:ln>"#.to_vec(),
        )?;
        assert_eq!(parse_line(&no_fill, &theme, &mut doc), Edge::Hidden);

        // w="0" -> Hidden
        let zero = xml::parse(
            br#"<a:ln w="0" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"/>"#
                .to_vec(),
        )?;
        assert_eq!(parse_line(&zero, &theme, &mut doc), Edge::Hidden);

        // 无色仅 w：颜色回退主题文字色
        let width_only = xml::parse(
            br#"<a:ln w="25400" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"/>"#
                .to_vec(),
        )?;
        match parse_line(&width_only, &theme, &mut doc) {
            Edge::Line(border) => {
                assert_eq!(border.color, theme.text_color());
                assert!((border.width - 2.0).abs() < 1e-4);
            }
            other => panic!("expected visible line, got {other:?}"),
        }

        // 非实线 prstDash 降级为实线并提示
        let dashed = xml::parse(
            br#"<a:ln w="12700" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
                <a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:prstDash val="dash"/>
            </a:ln>"#
                .to_vec(),
        )?;
        let mut dash_doc = Document::new(Kind::PPTX);
        assert!(matches!(
            parse_line(&dashed, &theme, &mut dash_doc),
            Edge::Line(_)
        ));
        assert!(dash_doc
            .warnings
            .iter()
            .any(|w| w.contains("dashed table cell borders")));
        Ok(())
    }

    /// 未知 tableStyleId 必须告警，且 `<a:tblBg>` 不能被丢弃。
    #[test]
    fn unknown_style_id_warns_and_keeps_table_background() -> crate::Result<()> {
        let table_xml = build_table("", "{MISSING}", 2, 2)?;
        // tblBg 与 tableStyleId 同为 <a:tblPr> 的子元素，构造一个带底纹且引用未知样式的表
        let grid = r#"<a:gridCol w="1000000"/><a:gridCol w="1000000"/>"#;
        let body = r#"<a:tr><a:tc><a:tcPr/></a:tc><a:tc><a:tcPr/></a:tc></a:tr>
                     <a:tr><a:tc><a:tcPr/></a:tc><a:tc><a:tcPr/></a:tc></a:tr>"#;
        let with_bg = xml::parse(
            format!(
                r#"<a:tbl xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
                    <a:tblPr>
                        <a:tblBg><a:solidFill><a:srgbClr val="ABCDEF"/></a:solidFill></a:tblBg>
                        <a:tableStyleId>{{MISSING}}</a:tableStyleId>
                    </a:tblPr><a:tblGrid>{grid}</a:tblGrid>{body}
                </a:tbl>"#
            )
            .into_bytes(),
        )?;
        let styles = r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <a:tblStyle styleId="{OTHER}"/>
        </a:tblStyleLst>"#;
        let styles_node = xml::parse(styles.as_bytes().to_vec())?;
        let theme = theme()?;
        let mut budget = Budget::default();
        let mut doc = Document::new(Kind::PPTX);
        let resolved = resolve_table_style(
            &with_bg,
            2,
            2,
            Some(&styles_node),
            &theme,
            &mut budget,
            &mut doc,
        );

        assert!(doc
            .warnings
            .iter()
            .any(|w| w.contains("not defined in ppt/tableStyles.xml")));
        // tblBg 仍然生效
        assert_eq!(resolved.fills[0][0], Some(0xffab_cdef));
        assert_eq!(resolved.fills[1][1], Some(0xffab_cdef));

        // 无 tableStyles.xml 时也只告警一次并保留 tblBg
        let mut budget2 = Budget::default();
        let mut doc2 = Document::new(Kind::PPTX);
        let resolved2 =
            resolve_table_style(&table_xml, 2, 2, None, &theme, &mut budget2, &mut doc2);
        assert!(doc2
            .warnings
            .iter()
            .any(|w| w.contains("missing ppt/tableStyles.xml")));
        assert_eq!(resolved2.fills[0][0], None);
        Ok(())
    }
}
