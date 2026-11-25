use super::parts::Part;
use crate::model::Document;
use crate::xml::{self, Node};
use std::collections::HashMap;
use std::rc::Rc;

enum ThemeSource {
    Part(Rc<Part>),
    Node(Node),
}

impl ThemeSource {
    fn root(&self) -> &Node {
        match self {
            Self::Part(part) => &part.root,
            Self::Node(node) => node,
        }
    }
}

pub(crate) struct Theme {
    source: Option<ThemeSource>,
    colors: HashMap<String, u32>,
    mapping: HashMap<String, String>,
}

impl Default for Theme {
    fn default() -> Self {
        let mut colors = HashMap::new();
        colors.insert("dk1".to_owned(), 0xff00_0000);
        colors.insert("lt1".to_owned(), 0xffff_ffff);
        let mut mapping = HashMap::new();
        for (key, value) in [
            ("tx1", "dk1"),
            ("bg1", "lt1"),
            ("tx2", "dk2"),
            ("bg2", "lt2"),
        ] {
            mapping.insert(key.to_owned(), value.to_owned());
        }
        Self {
            source: None,
            colors,
            mapping,
        }
    }
}

impl Theme {
    pub(super) fn new(source: Option<Rc<Part>>, doc: &mut Document) -> Self {
        Self::init(source.map(ThemeSource::Part), doc)
    }

    pub(crate) fn from_node(root: Option<&Node>, doc: &mut Document) -> Self {
        Self::init(root.cloned().map(ThemeSource::Node), doc)
    }

    fn init(source: Option<ThemeSource>, doc: &mut Document) -> Self {
        let mut result = Self {
            source,
            ..Self::default()
        };
        if let Some(scheme) = result
            .source
            .as_ref()
            .and_then(|s| s.root().child("themeElements"))
            .and_then(|n| n.child("clrScheme"))
        {
            for entry in &scheme.children {
                let color = result.color(entry, 0xff20_2124, doc);
                result.colors.insert(entry.name.clone(), color);
            }
        }
        result
    }

    pub fn set_mapping(&mut self, slide: &Node, layout: Option<&Node>, master: Option<&Node>) {
        self.map(master.and_then(|n| n.child("clrMap")));
        let slide_override = slide.child("clrMapOvr");
        if slide_override
            .and_then(|n| n.child("masterClrMapping"))
            .is_none()
        {
            self.map(
                layout
                    .and_then(|n| n.child("clrMapOvr"))
                    .and_then(|n| n.child("overrideClrMapping")),
            );
        }
        self.map(slide_override.and_then(|n| n.child("overrideClrMapping")));
    }

    fn map(&mut self, node: Option<&Node>) {
        if let Some(node) = node {
            self.mapping.extend(node.attrs.iter().cloned());
        }
    }

    fn resolve(&self, key: &str) -> Option<u32> {
        self.colors
            .get(self.mapping.get(key).map_or(key, String::as_str))
            .copied()
    }

    pub fn text_color(&self) -> u32 {
        self.resolve("tx1").unwrap_or(0xff202124)
    }

    /// 按 clrScheme 槽位名直接取主题色，不做 tx1/bg1 别名映射。
    /// DOCX 的 `w:themeColor` / `w:themeFill` 直接引用槽位名（accent1、dk1 等），故单独提供。
    pub(crate) fn style_color(&self, key: &str) -> Option<u32> {
        self.colors.get(key).copied()
    }

    pub fn accent(&self, index: usize) -> u32 {
        const FALLBACK: [u32; 6] = [
            0xff4472c4, 0xffed7d31, 0xffa5a5a5, 0xffffc000, 0xff5b9bd5, 0xff70ad47,
        ];
        self.resolve(&format!("accent{}", index % 6 + 1))
            .unwrap_or(FALLBACK[index % 6])
    }

    pub fn color(&self, parent: &Node, fallback: u32, doc: &mut Document) -> u32 {
        for node in &parent.children {
            let value = match node.name.as_str() {
                "srgbClr" => xml::color(node.attr("val"), fallback),
                "sysClr" => xml::color(node.attr("lastClr"), fallback),
                "schemeClr" => match self.resolve(node.attr("val")) {
                    Some(value) => value,
                    None => {
                        if node.attr("val") != "phClr" {
                            doc.warn("Some PPTX theme colors could not be resolved.");
                        }
                        fallback
                    }
                },
                "prstClr" | "hslClr" | "scrgbClr" => {
                    doc.warn("PPTX preset, HSL, and scRGB colors are not supported.");
                    return fallback;
                }
                _ => continue,
            };
            return transforms(value, node, doc);
        }
        fallback
    }

    pub fn fill(&self, properties: &Node, fallback: u32, doc: &mut Document) -> u32 {
        if properties.child("noFill").is_some() {
            return 0;
        }
        if let Some(solid) = properties.child("solidFill") {
            return self.color(solid, fallback, doc);
        }
        if ["gradFill", "pattFill", "blipFill", "grpFill"]
            .iter()
            .any(|n| properties.child(n).is_some())
        {
            doc.warn("PPTX gradient, pattern, picture, and group fills are not supported.");
        }
        fallback
    }

    pub fn format_list(&self, name: &str) -> Option<&Node> {
        self.source
            .as_ref()?
            .root()
            .child("themeElements")?
            .child("fmtScheme")?
            .child(name)
    }

    /// 从主题 `fontScheme` 解析 `major` / `minor` 字体族名，优先按脚本区分中西文。
    ///
    /// `key` 为 `"major"` 或 `"minor"`（对应 `<a:majorFont>` / `<a:minorFont>`）。
    /// `east_asian` 为真时优先取 `<a:ea typeface="...">`，其次取 `script="Hans"` 的
    /// `<a:font>`，最后回退到 `<a:latin>`；为假时直接取 `<a:latin>`。
    /// 主题缺失、分组缺失或对应 `typeface` 为空串时返回 `None`，由调用方沿用平台默认字体。
    pub(crate) fn font_face(&self, key: &str, east_asian: bool) -> Option<&str> {
        let group_name = match key {
            "major" => "majorFont",
            "minor" => "minorFont",
            _ => return None,
        };
        let group = self
            .source
            .as_ref()?
            .root()
            .child("themeElements")?
            .child("fontScheme")?
            .child(group_name)?;
        let latin = group
            .child("latin")
            .map(|n| n.attr("typeface"))
            .filter(|s| !s.is_empty());
        if east_asian {
            let ea = group
                .child("ea")
                .map(|n| n.attr("typeface"))
                .filter(|s| !s.is_empty());
            let hans = group
                .named("font")
                .find(|n| n.attr("script") == "Hans")
                .map(|n| n.attr("typeface"))
                .filter(|s| !s.is_empty());
            return ea.or(hans).or(latin);
        }
        latin
    }

    pub fn fill_reference(&self, reference: &Node, fallback: u32, doc: &mut Document) -> u32 {
        let index = reference.attr("idx").parse::<usize>().unwrap_or(0);
        let (list, offset) = match index {
            0 | 1000 => return 0,
            1001.. => ("bgFillStyleLst", index - 1001),
            _ => ("fillStyleLst", index - 1),
        };
        let placeholder = self.color(reference, fallback, doc);
        if let Some(style) = self.format_list(list).and_then(|n| n.children.get(offset)) {
            match style.name.as_str() {
                "solidFill" => return self.color(style, placeholder, doc),
                "noFill" => return 0,
                // 表格与主题 fillRef 常引用渐变填充；取首个色标做纯色兜底，避免整行表头变白
                "gradFill" => {
                    if let Some(stop) = style.child("gsLst").and_then(|gs| gs.named("gs").next()) {
                        doc.warn("Some PPTX theme fill styles use solid fallback colors.");
                        return self.color(stop, placeholder, doc);
                    }
                    if placeholder != 0 {
                        doc.warn("Some PPTX theme fill styles use solid fallback colors.");
                        return placeholder;
                    }
                    doc.warn("Some PPTX theme fill styles are not supported.");
                    return fallback;
                }
                _ => (),
            }
        }
        doc.warn("Some PPTX theme fill styles are not supported.");
        fallback
    }
}

fn transforms(value: u32, node: &Node, doc: &mut Document) -> u32 {
    let mut channels = [(value >> 16) & 255, (value >> 8) & 255, value & 255].map(f64::from);
    let mut alpha = value >> 24;
    let mut luminance = None;
    for transform in &node.children {
        if matches!(transform.name.as_str(), "lumMod" | "lumOff") {
            let Some(amount) = transform
                .attr("val")
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map(|value| value / 100_000.0)
            else {
                doc.warn("Some PPTX luminance transforms are invalid.");
                continue;
            };
            let lightness = luminance.unwrap_or_else(|| {
                (channels.into_iter().fold(255.0, f64::min)
                    + channels.into_iter().fold(0.0, f64::max))
                    / 510.0
            });
            luminance = Some(if transform.name == "lumMod" {
                (lightness * amount).clamp(0.0, 1.0)
            } else {
                (lightness + amount).clamp(0.0, 1.0)
            });
            continue;
        }
        let amount = transform
            .attr("val")
            .parse::<u32>()
            .unwrap_or(100_000)
            .min(100_000);
        match transform.name.as_str() {
            "alpha" => alpha = (255 * amount + 50_000) / 100_000,
            "alphaMod" => alpha = (alpha * amount + 50_000) / 100_000,
            "shade" => {
                apply_luminance(&mut channels, luminance.take());
                for channel in &mut channels {
                    *channel = (*channel * f64::from(amount) / 100_000.0).round();
                }
            }
            "tint" => {
                apply_luminance(&mut channels, luminance.take());
                for channel in &mut channels {
                    *channel = 255.0 - ((255.0 - *channel) * f64::from(amount) / 100_000.0).round();
                }
            }
            _ => doc.warn("Some PPTX color transforms are not supported."),
        }
    }
    apply_luminance(&mut channels, luminance);
    let [red, green, blue] = channels.map(|channel| channel.round() as u32);
    (alpha << 24) | (red << 16) | (green << 8) | blue
}

fn apply_luminance(channels: &mut [f64; 3], lightness: Option<f64>) {
    let Some(lightness) = lightness else { return };
    let min = channels.iter().copied().fold(255.0, f64::min) / 255.0;
    let max = channels.iter().copied().fold(0.0, f64::max) / 255.0;
    if max == min {
        *channels = [lightness * 255.0; 3];
        return;
    }
    // Keep the original hue and saturation through successive luminance transforms.
    let saturation = (max - min) / (1.0 - (max + min - 1.0).abs());
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    for channel in channels {
        *channel = (((*channel / 255.0 - min) / (max - min)) * chroma + lightness - chroma / 2.0)
            .clamp(0.0, 1.0)
            * 255.0;
    }
}

#[cfg(test)]
mod tests {
    use super::Theme;
    use crate::model::{Document, Kind};
    use crate::xml;

    #[test]
    fn font_face_and_gradient_fill_reference_resolve_from_theme() -> crate::Result<()> {
        let theme_xml = xml::parse(
            br#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
                <a:themeElements>
                    <a:clrScheme name="C">
                        <a:dk1><a:srgbClr val="101010"/></a:dk1>
                        <a:lt1><a:srgbClr val="FFFFFF"/></a:lt1>
                        <a:accent1><a:srgbClr val="2A9D8F"/></a:accent1>
                    </a:clrScheme>
                    <a:fontScheme name="F">
                        <a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:font script="Hans" typeface="SimHei"/></a:majorFont>
                        <a:minorFont><a:latin typeface="Calibri"/><a:ea typeface="DengXian"/></a:minorFont>
                    </a:fontScheme>
                    <a:fmtScheme name="Fmt">
                        <a:fillStyleLst>
                            <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
                            <a:gradFill><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"><a:tint val="50000"/></a:schemeClr></a:gs></a:gsLst></a:gradFill>
                        </a:fillStyleLst>
                    </a:fmtScheme>
                </a:themeElements>
            </a:theme>"#.to_vec(),
        )?;
        let mut doc = Document::new(Kind::PPTX);
        let theme = Theme::from_node(Some(&theme_xml), &mut doc);
        assert_eq!(theme.font_face("major", false), Some("Calibri Light"));
        assert_eq!(theme.font_face("major", true), Some("SimHei"));
        assert_eq!(theme.font_face("minor", true), Some("DengXian"));

        let fill_ref = xml::parse(
            br#"<a:fillRef idx="2" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
                <a:schemeClr val="accent1"/>
            </a:fillRef>"#
                .to_vec(),
        )?;
        let resolved = theme.fill_reference(&fill_ref, 0, &mut doc);
        // 渐变兜底不能退化成透明（0），也不能等同于未经 tint 的 accent1
        assert_ne!(resolved, 0);
        assert_ne!(resolved, 0xff2a_9d8f);
        Ok(())
    }

    /// 未知 fontScheme 分组与西文回退：`major` 的 `ea` 为空串时必须回退到 `latin`
    #[test]
    fn font_face_falls_back_to_latin_and_rejects_unknown_keys() -> crate::Result<()> {
        let theme_xml = xml::parse(
            br#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
                <a:themeElements>
                    <a:clrScheme name="C"><a:dk1><a:srgbClr val="000000"/></a:dk1></a:clrScheme>
                    <a:fontScheme name="F">
                        <a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/></a:majorFont>
                    </a:fontScheme>
                </a:themeElements>
            </a:theme>"#.to_vec(),
        )?;
        let mut doc = Document::new(Kind::PPTX);
        let theme = Theme::from_node(Some(&theme_xml), &mut doc);
        // major 的 ea 为空串，应回退 latin
        assert_eq!(theme.font_face("major", true), Some("Calibri Light"));
        // 未声明的 minorFont 返回 None
        assert_eq!(theme.font_face("minor", false), None);
        // 非 major/minor 的 key 直接拒绝
        assert_eq!(theme.font_face("other", false), None);
        Ok(())
    }
}
