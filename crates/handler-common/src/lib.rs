pub mod attribute_filter;
pub mod document_handler;
pub mod document_issue;
pub mod document_node;
pub mod extended_properties;
pub mod find_replace;
pub mod hyperlink_validator;
pub mod insert_position;
pub mod mutation_selector_guard;
pub mod output_format;
pub mod path_aliases;
pub mod path_range;
pub mod path_segment;
pub mod selector;
pub mod style_unsupported_hints;
pub mod text_map;
pub mod validation_error;

pub use attribute_filter::{filter_nodes_by_selector, AttributePredicate, PredicateOp};
pub use document_handler::{DocumentHandler, HandlerError, MergeResult};
pub use document_issue::{DocumentIssue, IssueSeverity};
pub use document_node::DocumentNode;
pub use find_replace::{
    extract_find_replace_props, find_all_offsets, find_all_replacements, find_all_spans,
    find_replace_property_keys, replace_in_string, FindReplaceOptions, FindReplaceResult,
};
pub use insert_position::InsertPosition;
pub use mutation_selector_guard::{ensure_scoped, ensure_scoped_or_known_global};
pub use output_format::{BinaryInfo, OutputFormat, RawOptions, ViewOptions};
pub use path_aliases::PathAliases;
pub use path_range::{parse_range_paths, PathRangeSegment};
pub use path_segment::PathSegment;
pub use selector::Selector;
pub use style_unsupported_hints::{
    format as format_style_hint, suggest_property, KNOWN_STYLE_PROPS,
};
pub use text_map::{BBoxSpan, OffsetSpan, StyleSpan, TextMapMeta, TextOffsetMap};
pub use validation_error::ValidationError;
