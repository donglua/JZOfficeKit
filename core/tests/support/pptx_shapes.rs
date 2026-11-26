pub const GROUP: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/>
<a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

pub fn title(text: &str, color: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr>
<p:spPr><a:xfrm><a:off x="457200" y="304800"/><a:ext cx="6350000" cy="685800"/></a:xfrm>
<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln><a:noFill/></a:ln></p:spPr>
<p:txBody><a:bodyPr lIns="0" tIns="0" rIns="0" bIns="0"/><a:lstStyle/>
<a:p><a:pPr><a:defRPr sz="2800" b="1"/></a:pPr><a:r><a:rPr lang="en-US" sz="2800" b="1">
<a:solidFill><a:srgbClr val="{color}"/></a:solidFill></a:rPr><a:t>{text}</a:t></a:r>
<a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp>"#
    )
}

pub const GEOMETRY: &str = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Rectangle"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
<p:spPr><a:xfrm><a:off x="457200" y="1422400"/><a:ext cx="2286000" cy="762000"/></a:xfrm>
<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="DCFCE7"/></a:solidFill>
<a:ln w="25400"><a:solidFill><a:srgbClr val="228855"/></a:solidFill></a:ln></p:spPr></p:sp>
<p:sp><p:nvSpPr><p:cNvPr id="4" name="Ellipse"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
<p:spPr><a:xfrm><a:off x="3048000" y="1422400"/><a:ext cx="1016000" cy="635000"/></a:xfrm>
<a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FFE3A3"/></a:solidFill>
<a:ln w="12700"><a:solidFill><a:srgbClr val="9A6500"/></a:solidFill></a:ln></p:spPr></p:sp>
<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="5" name="Horizontal line"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr>
<p:spPr><a:xfrm><a:off x="457200" y="2413000"/><a:ext cx="3606800" cy="0"/></a:xfrm>
<a:prstGeom prst="line"><a:avLst/></a:prstGeom>
<a:ln w="25400"><a:solidFill><a:srgbClr val="475569"/></a:solidFill></a:ln></p:spPr></p:cxnSp>"#;

pub const IMAGE: &str = r#"<p:pic><p:nvPicPr><p:cNvPr id="6" name="Small teal PNG"/>
<p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr>
<p:blipFill><a:blip r:embed="rImage"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>
<p:spPr><a:xfrm><a:off x="5080000" y="1422400"/><a:ext cx="914400" cy="457200"/></a:xfrm>
<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#;

pub fn table() -> String {
    // 展示表样式级联与独立边框能力：
    // - tblPr 启用 firstRow / bandRow 并引用 tableStyles.xml 的 "Showcase" 样式（主题表头 + 斑马纹）；
    // - 第 2 行第 1 格用 tcPr 直设三线风格红色上粗线，演示独立四边边框覆盖表样式；
    // - 第 2 行第 2 格显式 marL/marR 归零，演示 DrawingML 内边距与平台默认 5pt 的差异；
    // - 每行给出不同 tr/@h 显式行高，验证平台侧行高下限。
    let mut text = String::from(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="7" name="Bilingual table"/>
<p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
<p:xfrm><a:off x="457200" y="2921000"/><a:ext cx="4572000" cy="914400"/></p:xfrm>
<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">
<a:tbl><a:tblPr firstRow="1" bandRow="1"><a:tableStyleId>{7C2F1E4B-3B21-4A2C-9E7E-11A6B0E3C5D1}</a:tableStyleId></a:tblPr>
<a:tblGrid><a:gridCol w="1828800"/><a:gridCol w="2743200"/></a:tblGrid>"#,
    );
    for (height, cells) in [
        ("457200", [("Language", ""), ("Greeting", "")]),
        (
            "411480",
            [
                ("&#x4E2D;&#x6587;", ""),
                ("Hello &#x4F60;&#x597D;", r#"marL="0" marR="0""#),
            ],
        ),
    ] {
        text.push_str(&format!(r#"<a:tr h="{height}">"#));
        for (cell, properties) in cells {
            // 第 2 行第 1 格直设 1.5pt 强调色上边框，演示局部独立边框覆盖表样式
            let border = if cell == "&#x4E2D;&#x6587;" {
                r#"<a:lnT w="19050"><a:solidFill><a:srgbClr val="E76F51"/></a:solidFill></a:lnT>"#
            } else {
                ""
            };
            text.push_str(&format!(
                r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>
<a:p><a:r><a:rPr lang="en-US" sz="1600"/><a:t>{cell}</a:t></a:r></a:p></a:txBody><a:tcPr {properties}>{border}</a:tcPr></a:tc>"#
            ));
        }
        text.push_str("</a:tr>");
    }
    text.push_str("</a:tbl></a:graphicData></a:graphic></p:graphicFrame>");
    text
}

/// `ppt/tableStyles.xml` 中供综合样例表格引用的表样式定义。
///
/// 表头（firstRow）使用强调色底与浅色粗体字，数据区启用隔行底纹，wholeTbl 给出外框与内部细线。
pub fn table_styles() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
def="{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}">
<a:tblStyle styleId="{7C2F1E4B-3B21-4A2C-9E7E-11A6B0E3C5D1}" styleName="Showcase">
<a:wholeTbl><a:tcStyle><a:tcBdr>
<a:top><a:ln w="12700"><a:solidFill><a:schemeClr val="accent4"/></a:solidFill></a:ln></a:top>
<a:bottom><a:ln w="12700"><a:solidFill><a:schemeClr val="accent4"/></a:solidFill></a:ln></a:bottom>
<a:left><a:ln w="12700"><a:solidFill><a:schemeClr val="accent4"/></a:solidFill></a:ln></a:left>
<a:right><a:ln w="12700"><a:solidFill><a:schemeClr val="accent4"/></a:solidFill></a:ln></a:right>
<a:insideH><a:ln w="6350"><a:solidFill><a:schemeClr val="accent6"/></a:solidFill></a:ln></a:insideH>
<a:insideV><a:ln w="6350"><a:solidFill><a:schemeClr val="accent6"/></a:solidFill></a:ln></a:insideV>
</a:tcBdr></a:tcStyle></a:wholeTbl>
<a:band1H><a:tcStyle><a:fill><a:solidFill><a:schemeClr val="lt2"/></a:solidFill></a:fill></a:tcStyle></a:band1H>
<a:band2H><a:tcStyle><a:fill><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:fill></a:tcStyle></a:band2H>
<a:firstRow>
<a:tcStyle><a:fill><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:fill></a:tcStyle>
<a:tcTxStyle b="on"><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></a:tcTxStyle>
</a:firstRow>
</a:tblStyle>
</a:tblStyleLst>"#
        .to_owned()
}
