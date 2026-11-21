use super::Parser;
use crate::model::Document;
use crate::xml::Node;
use crate::{Error, Package, Result};
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub(super) enum NoteKind {
    Footnote,
    Endnote,
}

impl NoteKind {
    fn relationship(self) -> &'static str {
        match self {
            Self::Footnote => "footnotes",
            Self::Endnote => "endnotes",
        }
    }

    fn root_name(self) -> &'static str {
        self.relationship()
    }

    fn item_name(self) -> &'static str {
        match self {
            Self::Footnote => "footnote",
            Self::Endnote => "endnote",
        }
    }
}

struct Note {
    node: Option<Node>,
    number: Option<u32>,
    rendered: bool,
}

#[derive(Default)]
pub(super) struct NoteCollection {
    relationships: HashMap<String, String>,
    notes: HashMap<String, Note>,
    next_number: u32,
    rendered_count: usize,
}

impl NoteCollection {
    pub(super) fn load(
        package: &mut Package,
        main_part: &str,
        kind: NoteKind,
        document: &mut Document,
    ) -> Result<Self> {
        let Some(path) = package.related_by_type(main_part, kind.relationship())? else {
            return Ok(Self::default());
        };
        let Some(root) = package.optional_xml(&path)? else {
            document.warn("DOCX note relationship target is missing; notes are omitted.");
            return Ok(Self::default());
        };
        if root.name != kind.root_name() {
            return Err(Error::Invalid("Invalid DOCX notes root element"));
        }
        let relationships = package.relationships(&path)?;
        let mut collection = Self {
            relationships,
            ..Self::default()
        };
        for node in root.children {
            if node.name != kind.item_name() || !matches!(node.attr("type"), "" | "normal") {
                continue;
            }
            let id = node.attr("id").to_owned();
            if id.is_empty() {
                document.warn("DOCX notes without IDs are omitted.");
                continue;
            }
            let note = Note {
                node: Some(node),
                number: None,
                rendered: false,
            };
            if collection.notes.insert(id, note).is_some() {
                return Err(Error::Invalid("Duplicate DOCX note ID"));
            }
        }
        Ok(collection)
    }

    /// 正文首次引用时分配连续显示编号，避免把包内任意 ID 直接当作脚注编号。
    pub(super) fn reference(&mut self, id: &str) -> Option<u32> {
        let note = self.notes.get_mut(id)?;
        if let Some(number) = note.number {
            return Some(number);
        }
        self.next_number = self.next_number.saturating_add(1);
        let number = self.next_number;
        note.number = Some(number);
        Some(number)
    }

    fn take_next(&mut self) -> Option<(u32, Node, bool)> {
        let id = self
            .notes
            .iter()
            .filter(|(_, note)| note.number.is_some() && !note.rendered)
            .min_by_key(|(_, note)| note.number)
            .map(|(id, _)| id.clone())?;
        let note = self.notes.get_mut(&id)?;
        note.rendered = true;
        let number = note.number?;
        let node = note.node.take()?;
        let first = self.rendered_count == 0;
        self.rendered_count += 1;
        Some((number, node, first))
    }
}

impl Parser<'_> {
    pub(super) fn note_collection_mut(&mut self, kind: NoteKind) -> &mut NoteCollection {
        match kind {
            NoteKind::Footnote => &mut self.footnotes,
            NoteKind::Endnote => &mut self.endnotes,
        }
    }

    /// 注释按静态连续流追加在正文之后，不模拟分页底部定位。
    pub(super) fn parse_notes(&mut self) -> Result<()> {
        self.parse_note_collection(NoteKind::Footnote)?;
        self.parse_note_collection(NoteKind::Endnote)
    }

    fn parse_note_collection(&mut self, kind: NoteKind) -> Result<()> {
        loop {
            if self.out_of_budget() {
                return Ok(());
            }
            let Some((number, node, first)) = self.note_collection_mut(kind).take_next() else {
                return Ok(());
            };
            let relationships = self.note_collection_mut(kind).relationships.clone();
            let saved_relationships = std::mem::replace(&mut self.relationships, relationships);
            let saved_note = self.active_note.replace(number);
            self.last_style_id = None;
            self.last_contextual = false;
            let first_block = self.document.blocks.len();
            super::inspect(&node, &mut self.document);
            let result = self.body(&node);
            self.active_note = saved_note;
            self.relationships = saved_relationships;
            result?;
            if first {
                separate_notes(&mut self.document, first_block);
            }
        }
    }
}

fn separate_notes(document: &mut Document, first_block: usize) {
    document
        .warn("DOCX footnotes and endnotes use continuous flow and automatic decimal numbering.");
    for block in &mut document.blocks[first_block..] {
        let Some(paragraph) = block.paragraphs.first_mut() else {
            continue;
        };
        paragraph.before = paragraph.before.max(12.0);
        if paragraph.border_top == 0 {
            paragraph.border_top = 0xffdadce0;
            paragraph.border_width = paragraph.border_width.max(0.5);
        }
        break;
    }
}
