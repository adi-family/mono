// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::types::Location;
use tree_sitter::Node;

/// Shared helper functions for all analyzers
#[derive(Debug)]
pub struct AnalyzerBase;

impl AnalyzerBase {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    #[must_use]
    pub fn node_text(&self, node: Node, source: &str) -> String {
        source[node.byte_range()].to_string()
    }

    #[must_use]
    pub fn node_location(&self, node: Node) -> Location {
        let start = node.start_position();
        let end = node.end_position();
        Location {
            start_line: start.row as u32,
            start_col: start.column as u32,
            end_line: end.row as u32,
            end_col: end.column as u32,
            start_byte: node.start_byte() as u32,
            end_byte: node.end_byte() as u32,
        }
    }

    #[must_use]
    pub fn find_child_by_field<'a>(&self, node: Node<'a>, field: &str) -> Option<Node<'a>> {
        node.child_by_field_name(field)
    }

    #[must_use]
    pub fn extract_doc_comment(&self, node: Node, source: &str) -> Option<String> {
        let mut prev = node.prev_sibling();
        let mut comments = Vec::new();

        while let Some(sibling) = prev {
            if sibling.kind() == "line_comment" || sibling.kind() == "block_comment" {
                let text = self.node_text(sibling, source);
                if text.starts_with("///") || text.starts_with("//!") || text.starts_with("/**") {
                    comments.push(text);
                } else {
                    break;
                }
            } else if sibling.kind() == "attribute_item" || sibling.kind() == "inner_attribute_item"
            {
                // Skip attributes
            } else {
                break;
            }
            prev = sibling.prev_sibling();
        }

        if comments.is_empty() {
            None
        } else {
            comments.reverse();
            Some(
                comments
                    .iter()
                    .map(|c| c.trim_start_matches("///").trim_start_matches("//!").trim())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
    }

    #[must_use]
    pub fn extract_function_signature(&self, node: Node, source: &str) -> String {
        let text = self.node_text(node, source);
        if let Some(brace_pos) = text.find('{') {
            text[..brace_pos].trim().to_string()
        } else if let Some(semi_pos) = text.find(';') {
            text[..semi_pos].trim().to_string()
        } else {
            text.lines().next().unwrap_or("").to_string()
        }
    }

    #[must_use]
    pub fn is_node_or_descendant(&self, ancestor: Node, target: Node) -> bool {
        Self::is_node_or_descendant_helper(ancestor, target)
    }

    fn is_node_or_descendant_helper(ancestor: Node, target: Node) -> bool {
        if ancestor.id() == target.id() {
            return true;
        }
        for i in 0..ancestor.child_count() {
            if let Some(child) = ancestor.child(i)
                && Self::is_node_or_descendant_helper(child, target) {
                    return true;
                }
        }
        false
    }
}

impl Default for AnalyzerBase {
    fn default() -> Self {
        Self::new()
    }
}
