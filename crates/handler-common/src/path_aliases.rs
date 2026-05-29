use std::collections::HashMap;

/// Maps human-friendly path segment names to their OpenXML local names.
/// Allows paths like /body/paragraph[1] in addition to /body/p[1].
pub struct PathAliases {
    aliases: HashMap<String, String>,
}

impl PathAliases {
    pub fn new() -> Self {
        let mut aliases = HashMap::new();
        // Word
        aliases.insert("paragraph".to_string(), "p".to_string());
        aliases.insert("run".to_string(), "r".to_string());
        aliases.insert("table".to_string(), "tbl".to_string());
        aliases.insert("row".to_string(), "tr".to_string());
        aliases.insert("cell".to_string(), "tc".to_string());
        aliases.insert("hyperlink".to_string(), "hyperlink".to_string());
        // PowerPoint
        aliases.insert("slide".to_string(), "slide".to_string());
        aliases.insert("shape".to_string(), "shape".to_string());
        aliases.insert("textbox".to_string(), "textbox".to_string());
        aliases.insert("picture".to_string(), "picture".to_string());
        // Excel
        aliases.insert("sheet".to_string(), "sheet".to_string());
        Self { aliases }
    }

    /// Resolve a path segment name to its canonical OpenXML local name.
    /// Returns the original name if no alias is defined.
    pub fn resolve(&self, name: &str) -> String {
        self.aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }
}

impl Default for PathAliases {
    fn default() -> Self {
        Self::new()
    }
}
