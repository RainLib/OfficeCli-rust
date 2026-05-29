/// Represents where to insert an element: by index, after an anchor, or before an anchor.
/// At most one field is set. None = append to end.
#[derive(Debug, Clone)]
pub enum InsertPosition {
    AtIndex(usize),
    AfterElement(String),
    BeforeElement(String),
    Append,
}

impl InsertPosition {
    pub fn at_index(idx: usize) -> Self {
        Self::AtIndex(idx)
    }

    pub fn after_element(path: &str) -> Self {
        Self::AfterElement(path.to_string())
    }

    pub fn before_element(path: &str) -> Self {
        Self::BeforeElement(path.to_string())
    }

    pub fn append() -> Self {
        Self::Append
    }

    /// Resolve After/Before anchor to a 0-based index among children.
    /// anchor_finder: given the anchor path, returns the 0-based index of that element.
    /// child_count: total number of children.
    pub fn resolve(
        &self,
        anchor_finder: impl Fn(&str) -> usize,
        child_count: usize,
    ) -> Option<usize> {
        match self {
            InsertPosition::AtIndex(idx) => Some(*idx),
            InsertPosition::AfterElement(anchor) => {
                let anchor_idx = anchor_finder(anchor);
                if anchor_idx + 1 >= child_count {
                    None // append
                } else {
                    Some(anchor_idx + 1)
                }
            }
            InsertPosition::BeforeElement(anchor) => Some(anchor_finder(anchor)),
            InsertPosition::Append => None,
        }
    }
}
