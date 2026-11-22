use crate::{model::Document, xml::Node};

pub(super) struct AlternateBranch<'a> {
    pub node: &'a Node,
    pub lossless: bool,
}

fn is_supported_choice_prefix(prefix: &str) -> bool {
    matches!(prefix, "p" | "a" | "r" | "c" | "pic")
}

/// 按 OOXML Markup Compatibility 规则从 mc:AlternateContent 中择优选择已支持分支：
/// 优先选用 Requires 全部受支持的第一个 mc:Choice，否则回退到 mc:Fallback。
pub(super) fn select_alternate(node: &Node) -> Option<AlternateBranch<'_>> {
    for choice in node.named("Choice") {
        let requires: Vec<&str> = choice.attr("Requires").split_ascii_whitespace().collect();
        if requires
            .iter()
            .all(|prefix| is_supported_choice_prefix(prefix))
        {
            return Some(AlternateBranch {
                node: choice,
                lossless: true,
            });
        }
    }
    node.child("Fallback").map(|fallback| AlternateBranch {
        node: fallback,
        lossless: false,
    })
}

pub(super) fn warn(node: &Node, doc: &mut Document) {
    if node.name == "AlternateContent" {
        match select_alternate(node) {
            Some(branch) => {
                if !branch.lossless {
                    doc.warn(
                        "PPTX alternate content uses one saved representation; unsupported features may be lost.",
                    );
                }
                warn(branch.node, doc);
            }
            None => doc.warn("PPTX alternate content is omitted."),
        }
        return;
    }
    match node.name.as_str() {
        "relIds" => doc.warn("PPTX SmartArt is omitted."),
        "oMath" | "oMathPara" => doc.warn("PPTX mathematical equations are omitted."),
        "timing" => doc.warn("PPTX animations and timing are not supported."),
        "transition" => doc.warn("PPTX transitions are not supported."),
        "audioFile" | "videoFile" | "media" | "oleObj" => {
            doc.warn("PPTX multimedia and embedded objects are not supported.")
        }
        "effectLst" | "effectDag" | "scene3d" | "sp3d" => {
            if !node.children.is_empty() {
                doc.warn("PPTX shape effects and 3D styling are not supported.");
            }
        }
        _ => (),
    }
    for child in &node.children {
        warn(child, doc);
    }
}
