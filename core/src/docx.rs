mod cell_style;
mod drawings;
mod format;
mod notes;
pub(super) mod numbering;
mod paragraphs;
mod section;
mod styles;
mod tables;
pub(super) mod theme;

use crate::model::{Document, Element, ElementType, Kind, OutlineItem, WrapMode};
use crate::xml::{self, Node};
use crate::{Error, Package, Result};
use notes::{NoteCollection, NoteKind};
use numbering::Numbering;
use std::collections::HashMap;
use styles::Styles;

const MAX_BLOCKS: usize = 5000;
/// 单个页眉/页脚部件的块数量上限，必须与 Android DocumentDecoder 的校验保持一致，
/// 否则 core 解析成功但平台层解码会直接抛异常。
const MAX_SECTION_BLOCKS: usize = 500;
const MAX_CELLS: usize = 5000;
const MAX_COLUMNS: usize = 32;
const MAX_ROWS: usize = 500;
pub(crate) const MAX_COORDINATE: f32 = 1200.0;
/// Word 默认页边距（点），用于未声明 sectPr/pgMar 的文档
const DEFAULT_MARGIN: f32 = 72.0;
pub(super) const MAX_TEXT: usize = 8 * 1024 * 1024;

/// 当前解析目标的块数量上限及其超限提示
#[derive(Clone, Copy)]
struct BlockLimit {
    count: usize,
    message: &'static str,
}

const FLOW_LIMIT: BlockLimit = BlockLimit {
    count: MAX_BLOCKS,
    message: "DOCX exceeds 5000 flow blocks",
};
const SECTION_LIMIT: BlockLimit = BlockLimit {
    count: MAX_SECTION_BLOCKS,
    message: "DOCX header/footer exceeds 500 blocks",
};

pub(crate) fn parse(
    package: &mut Package,
    main_part: &str,
    scope: crate::Scope,
) -> Result<Document> {
    let root = package.xml(main_part)?;
    if root.name != "document" {
        return Err(Error::Invalid("Invalid DOCX document root"));
    }
    let body = root
        .child("body")
        .ok_or(Error::Invalid("Missing DOCX body"))?;
    let mut document = Document::new(Kind::DOCX);
    let sections = section::properties(body);
    let mut page_height = 792.0_f32;
    if let Some(&sect) = sections.last() {
        if let Some(size) = sections.iter().rev().find_map(|s| s.child("pgSz")) {
            document.width =
                (xml::number(size.attr("w"), 11900.0) / 20.0).clamp(300.0, MAX_COORDINATE);
            page_height =
                (xml::number(size.attr("h"), 15840.0) / 20.0).clamp(300.0, MAX_COORDINATE);
        }
        document.page_height = page_height;
        let margins_sect = sections
            .iter()
            .rev()
            .find(|s| s.child("pgMar").is_some())
            .copied()
            .unwrap_or(sect);
        let margins = section::page_margins(margins_sect, document.width);
        document.header_distance = margins.header;
        document.footer_distance = margins.footer;
        document.margin_top = margins.top;
        document.margin_bottom = margins.bottom;
        document.margin_left = margins.left;
        document.margin_right = margins.right;
    }
    inspect(body, &mut document);
    let relationships = package.relationships(main_part)?;
    let external_relationships = package.external_relationships(main_part)?;
    let mut styles = Styles::load(package, main_part, &mut document)?;
    let line_pitch = sections
        .iter()
        .rev()
        .map(|s| section::line_pitch(s))
        .find(|&p| p > 0.0)
        .unwrap_or(0.0);
    styles.line_pitch = line_pitch;
    let numbering_part = package.related_by_type(main_part, "numbering")?;
    let numbering_root = if let Some(path) = numbering_part {
        package.optional_xml(&path)?
    } else {
        None
    };
    let numbering = Numbering::parse(numbering_root.as_ref());
    let footnotes = NoteCollection::load(package, main_part, NoteKind::Footnote, &mut document)?;
    let endnotes = NoteCollection::load(package, main_part, NoteKind::Endnote, &mut document)?;
    let mut parser = Parser {
        package,
        document,
        relationships,
        external_relationships,
        styles,
        numbering,
        footnotes,
        endnotes,
        active_note: None,
        cells: 0,
        text_bytes: 0,
        block_limit: FLOW_LIMIT,
        last_style_id: None,
        last_contextual: false,
        page_height,
        budget: scope.block_budget(),
        collect_outline: true,
        last_outline_level: None,
        active_link: None,
        bookmarks: HashMap::new(),
        pending_next_page_break: false,
    };
    parser.body(body)?;
    parser.collect_outline = false;
    resolve_docx_bookmarks(&mut parser.document.blocks, &parser.bookmarks);
    parser.parse_notes()?;
    // 解析页眉页脚（在正文之后，避免影响正文的 blocks 计数）。
    // 多节文档中页眉页脚引用按节继承，使用按文档顺序收集的全部 sectPr。
    if !sections.is_empty() {
        parser.parse_sections(&sections)?;
    }
    Ok(parser.document)
}

struct Parser<'a> {
    package: &'a mut Package,
    document: Document,
    relationships: HashMap<String, String>,
    pub(super) external_relationships: HashMap<String, String>,
    styles: Styles,
    pub(super) numbering: Numbering,
    footnotes: NoteCollection,
    endnotes: NoteCollection,
    pub(super) active_note: Option<u32>,
    cells: usize,
    text_bytes: usize,
    block_limit: BlockLimit,
    pub(super) last_style_id: Option<String>,
    pub(super) last_contextual: bool,
    pub(super) page_height: f32,
    /// 剩余可解析的正文块预算；首屏缩略图只排版少量块即可返回
    pub(super) budget: Option<usize>,
    /// 仅在解析正文流时收集大纲目录（排除页眉/页脚/脚注与文本框）
    collect_outline: bool,
    pub(super) last_outline_level: Option<u8>,
    pub(super) active_link: Option<String>,
    pub(super) bookmarks: HashMap<String, usize>,
    /// 上一段末尾出现 `<w:br w:type="page"/>` 时置位，落到下一正文块首部
    pub(super) pending_next_page_break: bool,
}

impl Parser<'_> {
    fn body(&mut self, parent: &Node) -> Result<()> {
        if self.out_of_budget() {
            return Ok(());
        }
        for node in &parent.children {
            if self.out_of_budget() {
                break;
            }
            match node.name.as_str() {
                "p" => {
                    self.last_outline_level = None;
                    let blocks = self.paragraph(node, false)?;
                    let outline_level = if self.collect_outline {
                        self.last_outline_level
                    } else {
                        None
                    };
                    let mut title_raw = String::new();
                    let mut target_block_index = None;
                    for block in blocks {
                        if outline_level.is_some()
                            && block.kind == ElementType::TEXT
                            && block.wrap_mode == WrapMode::Inline
                        {
                            if target_block_index.is_none() {
                                target_block_index = Some(self.document.blocks.len());
                            }
                            for paragraph in &block.paragraphs {
                                for run in &paragraph.runs {
                                    title_raw.push_str(&run.text);
                                }
                            }
                        }
                        self.push(block)?;
                    }
                    if let (Some(level), Some(block_index)) = (outline_level, target_block_index) {
                        push_outline_item(
                            &mut self.document.outline,
                            level,
                            block_index,
                            &title_raw,
                        );
                    }
                }
                "tbl" => {
                    self.last_style_id = None;
                    self.last_contextual = false;
                    let mut table = self.table(node)?;
                    if self.pending_next_page_break {
                        table.page_break_before = true;
                        self.pending_next_page_break = false;
                    }
                    self.push(table)?;
                }
                "sdt" => {
                    if let Some(content) = node.child("sdtContent") {
                        self.body(content)?;
                    }
                }
                "AlternateContent" => {
                    if let Some(content) = alternate(node) {
                        self.body(content)?;
                    }
                }
                "pict" | "object" | "txbxContent" => {
                    if matches!(node.name.as_str(), "pict" | "object") {
                        for image in self.legacy_images(node) {
                            self.push(image)?;
                        }
                    }
                    if let Some(content) = node.descendant("txbxContent") {
                        let prev_collect = self.collect_outline;
                        self.collect_outline = false;
                        self.body(content)?;
                        self.collect_outline = prev_collect;
                    }
                }
                name if skipped(name) => (),
                _ => self.body(node)?,
            }
        }
        Ok(())
    }

    fn push(&mut self, element: Element) -> Result<()> {
        if self.document.blocks.len() >= self.block_limit.count {
            return Err(Error::Limit(self.block_limit.message));
        }
        self.document.blocks.push(element);
        Ok(())
    }

    /// 首屏预算是否已经用尽。用尽后停止继续解析正文，已解析的前缀足以生成封面。
    fn out_of_budget(&self) -> bool {
        self.budget
            .is_some_and(|budget| self.document.blocks.len() >= budget)
    }

    pub(super) fn effective_margin(&self, value: f32) -> f32 {
        if value > 0.0 {
            value
        } else {
            DEFAULT_MARGIN
        }
    }

    /// 流式正文的可用宽度：扣除左右页边距。未从 sectPr 读到边距时按 Word 默认 72pt 回退，
    /// 与 Android OfficeRenderer 计算 contentWidth 的方式保持一致。
    fn flow_width(&self) -> f32 {
        (self.document.width
            - self.effective_margin(self.document.margin_left)
            - self.effective_margin(self.document.margin_right))
        .max(1.0)
    }

    /// 解析各节的页眉页脚引用，把内容块写入 document.header_blocks / footer_blocks。
    /// 多节文档中，后一节未重复声明引用时按 OOXML 规则继承前一节的页眉页脚，
    /// 因此从最后一节向前查找最近一个声明了该类型引用的节。
    fn parse_sections(&mut self, sections: &[&Node]) -> Result<()> {
        if let Some(part) = self.section_part(sections, section::SectionKind::Header) {
            if self.package.has(&part) {
                let blocks = self.section_blocks(&part)?;
                if !blocks.is_empty() {
                    self.document.header_blocks = blocks;
                }
            }
        }
        if let Some(part) = self.section_part(sections, section::SectionKind::Footer) {
            if self.package.has(&part) {
                let blocks = self.section_blocks(&part)?;
                if !blocks.is_empty() {
                    self.document.footer_blocks = blocks;
                }
            }
        }
        Ok(())
    }

    /// 取指定类型页眉/页脚对应部件路径：从最后一节向前，第一个声明了该类型且关系可解析的节生效
    fn section_part(&self, sections: &[&Node], kind: section::SectionKind) -> Option<String> {
        for sect in sections.iter().rev() {
            let mut chosen: Option<(String, String)> = None;
            for (reference_kind, rel_id, ref_type) in section::references(sect) {
                if reference_kind != kind {
                    continue;
                }
                // 优先选择 default 类型；没有 default 时取第一个
                if chosen
                    .as_ref()
                    .is_none_or(|(_, chosen_type)| chosen_type != "default")
                {
                    chosen = Some((rel_id, ref_type));
                }
            }
            if let Some((rel_id, _)) = chosen {
                if let Some(target) = self.relationships.get(&rel_id) {
                    return Some(target.clone());
                }
            }
        }
        None
    }
}

/// 按文档顺序收集 body 下全部节属性，供元数据速读复用同一套节定位规则
pub(crate) fn sections(body: &Node) -> Vec<&Node> {
    section::properties(body)
}

struct AlternateBranch<'a> {
    node: &'a Node,
    lossless: bool,
}

fn is_lossless_choice_prefix(prefix: &str) -> bool {
    matches!(prefix, "w" | "wp" | "a" | "pic" | "r" | "wps" | "wpg")
}

fn is_supported_choice_prefix(prefix: &str) -> bool {
    is_lossless_choice_prefix(prefix) || matches!(prefix, "wp14" | "a14" | "w14" | "w15")
}

/// 按 OOXML Markup Compatibility 规则从 mc:AlternateContent 中择优选择已支持分支：
/// 优先选用 Requires 全部受支持的第一个 mc:Choice，否则回退到 mc:Fallback。
fn select_alternate(node: &Node) -> Option<AlternateBranch<'_>> {
    for choice in node.named("Choice") {
        let requires: Vec<&str> = choice.attr("Requires").split_ascii_whitespace().collect();
        if requires
            .iter()
            .all(|prefix| is_supported_choice_prefix(prefix))
        {
            let lossless = requires
                .iter()
                .all(|prefix| is_lossless_choice_prefix(prefix));
            return Some(AlternateBranch {
                node: choice,
                lossless,
            });
        }
    }
    node.child("Fallback").map(|fallback| AlternateBranch {
        node: fallback,
        lossless: false,
    })
}

pub(super) fn alternate(node: &Node) -> Option<&Node> {
    select_alternate(node).map(|branch| branch.node)
}

fn skipped(name: &str) -> bool {
    matches!(
        name,
        "pPr"
            | "rPr"
            | "sectPr"
            | "tblPr"
            | "tblGrid"
            | "trPr"
            | "tcPr"
            | "sdtPr"
            | "sdtEndPr"
            | "customXmlPr"
            | "del"
            | "moveFrom"
            | "instrText"
            | "oMath"
            | "oMathPara"
            | "pict"
            | "object"
            | "txbxContent"
            | "altChunk"
            | "rt"
            | "rp"
    )
}

fn inspect(node: &Node, document: &mut Document) {
    inspect_node(node, document, false);
}

fn inspect_node(node: &Node, document: &mut Document, in_drawing: bool) {
    if node.name == "AlternateContent" {
        match select_alternate(node) {
            Some(branch) => {
                if !branch.lossless {
                    document.warn(
                        "Alternate content uses one saved representation; unsupported features may be lost.",
                    );
                }
                inspect_node(branch.node, document, in_drawing);
            }
            None => {
                document.warn(
                    "Alternate content uses one saved representation; unsupported features may be lost.",
                );
            }
        }
        return;
    }
    let child_in_drawing = in_drawing || node.name == "drawing";
    let warning = match node.name.as_str() {
        "headerReference" | "footerReference" => None, // 已支持页眉页脚
        "footnoteReference" | "endnoteReference" => None, // 已支持脚注与尾注静态预览
        "oMath" | "oMathPara" => Some("Mathematical equations are omitted."),
        "pict" if is_simple_vml_image(node) => None,
        "pict" | "object" => Some(
            "Text box content is preserved in document flow; legacy drawings and box layout are omitted.",
        ),
        "txbxContent" if !in_drawing => Some(
            "Text box content is preserved in document flow; legacy drawings and box layout are omitted.",
        ),
        "altChunk" => Some("Embedded document content is omitted."),
        "ins" | "del" | "moveFrom" | "moveTo" => {
            Some("Tracked changes show inserted content and omit deleted content.")
        }
        "fldSimple" | "fldChar" => Some("Fields display saved results and are not recalculated."),
        "cols" if xml::number(node.attr("num"), 1.0) > 1.0 => {
            Some("Multiple columns are rendered in a single continuous flow.")
        }
        "color" if !node.attr("themeColor").is_empty() => {
            Some("Theme colors use saved colors or inherited fallbacks.")
        }
        "vertAlign" | "strike" | "dstrike" | "vanish" => {
            Some("Some advanced run formatting is not reproduced.")
        }
        "pageBreakBefore" => None,
        "effectLst" => Some("Image drawing effects are not reproduced."),
        _ => None,
    };
    if let Some(message) = warning {
        document.warn(message);
    }
    for child in &node.children {
        inspect_node(child, document, child_in_drawing);
    }
}

fn is_simple_vml_image(pict: &Node) -> bool {
    pict.descendant("imagedata").is_some()
        && pict.descendant("txbxContent").is_none()
        && pict.descendant("textbox").is_none()
        && !has_unhandled_vml(pict)
}

const MAX_OUTLINE_ITEMS: usize = 500;
const MAX_OUTLINE_TITLE_UTF16: usize = 256;

fn push_outline_item(
    outline: &mut Vec<OutlineItem>,
    level: u8,
    block_index: usize,
    raw_title: &str,
) {
    if outline.len() >= MAX_OUTLINE_ITEMS || !(1..=6).contains(&level) {
        return;
    }
    let Ok(block_index) = u32::try_from(block_index) else {
        return;
    };
    let mut normalized = String::new();
    for part in raw_title.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(part);
    }
    if normalized.is_empty() {
        return;
    }
    let mut utf16_count = 0_usize;
    let mut cut = normalized.len();
    for (idx, ch) in normalized.char_indices() {
        let len16 = ch.len_utf16();
        if utf16_count + len16 > MAX_OUTLINE_TITLE_UTF16 {
            cut = idx;
            break;
        }
        utf16_count += len16;
    }
    if cut < normalized.len() {
        normalized.truncate(cut);
        normalized = normalized.trim_end().to_owned();
    }
    if normalized.is_empty() {
        return;
    }
    outline.push(OutlineItem {
        title: normalized,
        level,
        block_index,
    });
}

fn resolve_docx_bookmarks(blocks: &mut [Element], bookmarks: &HashMap<String, usize>) {
    let resolve_link = |link: &mut Option<String>| {
        if let Some(target) = link.as_deref() {
            if let Some(name) = target.strip_prefix("#bookmark:") {
                if let Some(&idx) = bookmarks.get(name) {
                    *link = Some(format!("#block:{idx}"));
                } else {
                    *link = None;
                }
            }
        }
    };
    for block in blocks {
        resolve_link(&mut block.link);
        for p in &mut block.paragraphs {
            for run in &mut p.runs {
                resolve_link(&mut run.link);
            }
        }
        for row in &mut block.rows {
            for cell in row {
                for p in cell {
                    for run in &mut p.runs {
                        resolve_link(&mut run.link);
                    }
                }
            }
        }
    }
}

fn has_unhandled_vml(node: &Node) -> bool {
    if node.name == "shapetype" {
        return false;
    }
    if node
        .attr("style")
        .to_ascii_lowercase()
        .replace(' ', "")
        .contains("position:absolute")
    {
        return true;
    }
    if matches!(
        node.name.as_str(),
        "line" | "polyline" | "arc" | "curve" | "oval" | "roundrect" | "rect"
    ) && node.descendant("imagedata").is_none()
    {
        return true;
    }
    node.children.iter().any(has_unhandled_vml)
}
