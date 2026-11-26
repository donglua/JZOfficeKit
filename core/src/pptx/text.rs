use super::colors::Theme;
use super::inheritance::{level_defaults, text_style};
use super::numbering;
use super::text_style::{apply_autofit, TextStyle};
use super::Budget;
use crate::model::{Document, Paragraph, Run};
use crate::xml::Node;
use crate::{Error, Result};

/// 表格单元格由 `tableStyles.xml` `tcTxStyle` 注入的文本覆盖项。
///
/// 在演示默认与母版 `txStyles/otherStyle` 之后、单元格自身 `lstStyle`/`pPr`/`rPr`
/// 之前生效，确保表头浅色字不会被母版默认深色字抹回黑色，同时允许单元格直设属性继续覆盖。
#[derive(Debug, Clone, Default)]
pub(crate) struct RunOverride {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub color: Option<u32>,
    pub font_face: Option<String>,
}

pub(super) struct Text<'a> {
    pub doc: &'a mut Document,
    pub theme: &'a Theme,
    pub budget: &'a mut Budget,
    pub defaults: Option<&'a Node>,
    pub master: Option<&'a Node>,
    pub chain: &'a [&'a Node],
}

impl Text<'_> {
    pub fn read(&mut self, body: &Node) -> Result<Vec<Paragraph>> {
        read_paragraphs_inner(
            body,
            self.chain,
            self.defaults,
            self.master,
            self.theme,
            18.0,
            None,
            self.doc,
            &mut |bytes| self.budget.text(bytes),
        )
    }

    /// 携带 `tcTxStyle` 文本覆盖读取单元格正文。
    ///
    /// 优先级：`presentation/defaultTextStyle` 与 `master/txStyles/otherStyle` < `override_run`
    /// < 单元格 `txBody/lstStyle` < `pPr/defRPr` < `rPr` / `endParaRPr`。
    pub fn read_with_override(
        &mut self,
        body: &Node,
        override_run: &RunOverride,
    ) -> Result<Vec<Paragraph>> {
        read_paragraphs_inner(
            body,
            self.chain,
            self.defaults,
            self.master,
            self.theme,
            18.0,
            Some(override_run),
            self.doc,
            &mut |bytes| self.budget.text(bytes),
        )
    }
}

/// 解析一 `<xdr:txBody>` / `<p:txBody>` 下的段落，字号基线、继承链与文本预算由调用方指定。
///
/// PPTX 幻灯片通过 [`Text::read`] 传入 `18.0` 与母版/版式继承链；XLSX 工作表形状没有母版与版式，
/// 以 `default_font_size = 11.0`（Excel 形状文本默认字号）与单形状继承链调用。
pub(crate) fn read_drawing_paragraphs(
    body: &Node,
    shape: &Node,
    theme: &Theme,
    default_font_size: f32,
    doc: &mut Document,
    add_text_bytes: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Vec<Paragraph>> {
    read_paragraphs_inner(
        body,
        &[shape],
        None,
        None,
        theme,
        default_font_size,
        None,
        doc,
        add_text_bytes,
    )
}

#[allow(clippy::too_many_arguments)]
fn read_paragraphs_inner(
    body: &Node,
    chain: &[&Node],
    defaults_source: Option<&Node>,
    master: Option<&Node>,
    theme: &Theme,
    default_font_size: f32,
    override_run: Option<&RunOverride>,
    doc: &mut Document,
    add_text_bytes: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Vec<Paragraph>> {
    let mut output = Vec::new();
    let mut numbering = [0_u32; 9];
    for paragraph in body.named("p") {
        let direct = paragraph.child("pPr");
        let level = direct.map_or(0, paragraph_level);
        let mut defaults = Vec::new();
        level_defaults(defaults_source, level, &mut defaults);
        level_defaults(
            master
                .and_then(|n| n.child("txStyles"))
                .and_then(|n| n.child(text_style(chain))),
            level,
            &mut defaults,
        );
        // 演示默认与母版样式边界：tableStyles tcTxStyle 必须在这些低优先基线之后、
        // 单元格自身的 lstStyle / pPr 之前注入，避免母版深色字把表头浅色字冲掉。
        let master_count = defaults.len();
        for shape in chain {
            if let Some(reference) = shape.child("style").and_then(|n| n.child("fontRef")) {
                defaults.push(reference);
            }
            if let Some(shape_body) = shape.child("txBody") {
                level_defaults(shape_body.child("lstStyle"), level, &mut defaults);
                if !std::ptr::eq(shape_body, body) {
                    if let Some(properties) = shape_body
                        .named("p")
                        .filter_map(|p| p.child("pPr"))
                        .find(|p| paragraph_level(p) == level)
                    {
                        defaults.push(properties);
                    }
                }
            }
        }
        if chain.is_empty() {
            level_defaults(body.child("lstStyle"), level, &mut defaults);
        }
        if let Some(direct) = direct {
            defaults.push(direct);
        }
        let mut base = Run {
            size: default_font_size,
            color: theme.text_color(),
            ..Run::default()
        };
        let mut result = Paragraph {
            after: 0.0,
            ..Paragraph::default()
        };
        let mut bullet = None;
        let mut styles = TextStyle { doc, theme };
        let mut applied_override = false;
        for (index, properties) in defaults.into_iter().enumerate() {
            if index == master_count {
                apply_run_override(&mut base, override_run);
                applied_override = true;
            }
            if properties.name == "fontRef" {
                base.color = styles.theme.color(properties, base.color, styles.doc);
                continue;
            }
            styles.paragraph(&mut result, properties);
            styles.run(&mut base, properties.child("defRPr"));
            for property in &properties.children {
                if matches!(
                    property.name.as_str(),
                    "buNone" | "buChar" | "buAutoNum" | "buBlip"
                ) {
                    bullet = Some(property);
                }
            }
        }
        if !applied_override {
            apply_run_override(&mut base, override_run);
        }
        if let Some(bullet) = bullet {
            match bullet.name.as_str() {
                "buChar" => result.bullet = format!("{} ", bullet.attr("char")),
                "buAutoNum" => {
                    let counter = numbering
                        .get_mut(level)
                        .ok_or(Error::Invalid("Invalid PPTX list level"))?;
                    if *counter == 0 {
                        *counter = bullet
                            .attr("startAt")
                            .parse::<u32>()
                            .unwrap_or(1)
                            .clamp(1, 32767);
                    }
                    result.bullet = numbering::format(*counter, bullet.attr("type"))
                        .unwrap_or_else(|| {
                            styles.doc.warn("PPTX numbered lists use decimal numbers.");
                            format!("{counter}. ")
                        });
                    *counter += 1;
                }
                "buBlip" => styles.doc.warn("PPTX picture bullets are omitted."),
                _ => (),
            }
        }
        for counter in numbering.iter_mut().skip(level + 1) {
            *counter = 0;
        }
        add_text_bytes(result.bullet.len())?;
        collect_runs(
            paragraph,
            &base,
            &mut styles,
            add_text_bytes,
            &mut result.runs,
        )?;
        if result.runs.is_empty() {
            styles.run(&mut base, paragraph.child("endParaRPr"));
            result.runs.push(base);
        }
        output.push(result);
    }
    apply_autofit(&mut output, body, chain);
    Ok(output)
}

fn collect_runs(
    parent: &Node,
    base: &Run,
    styles: &mut TextStyle<'_>,
    add_text_bytes: &mut impl FnMut(usize) -> Result<()>,
    runs: &mut Vec<Run>,
) -> Result<()> {
    for item in &parent.children {
        if item.name == "AlternateContent" {
            if let Some(branch) = super::unsupported::select_alternate(item) {
                collect_runs(branch.node, base, styles, add_text_bytes, runs)?;
            }
            continue;
        }
        if !matches!(item.name.as_str(), "r" | "fld" | "br") {
            continue;
        }
        let mut run = base.clone();
        if item.name == "br" {
            run.text.push('\n');
        } else {
            for segment in item.named("t") {
                run.text.push_str(&segment.text);
            }
            if item.name == "fld" {
                styles.doc.warn("PPTX fields use saved text values.");
            }
        }
        styles.run(&mut run, item.child("rPr"));
        add_text_bytes(run.text.len())?;
        runs.push(run);
    }
    Ok(())
}

fn apply_run_override(base: &mut Run, override_run: Option<&RunOverride>) {
    let Some(override_run) = override_run else {
        return;
    };
    if let Some(bold) = override_run.bold {
        base.bold = bold;
    }
    if let Some(italic) = override_run.italic {
        base.italic = italic;
    }
    if let Some(color) = override_run.color {
        base.color = color;
    }
    if let Some(font_face) = &override_run.font_face {
        base.font_face = font_face.clone();
        base.weight = crate::model::infer_font_weight(&base.font_face);
    }
}

fn paragraph_level(node: &Node) -> usize {
    node.attr("lvl").parse::<usize>().unwrap_or(0).min(8)
}
