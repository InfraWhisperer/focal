pub mod go;
pub mod python;
pub mod rust_lang;
pub mod typescript;

use std::path::Path;

// ---------------------------------------------------------------------------
// Symbol kinds
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Class,
    Interface,
    Trait,
    TypeAlias,
    Const,
    Module,
    Enum,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Struct => "struct",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Trait => "trait",
            SymbolKind::TypeAlias => "type_alias",
            SymbolKind::Const => "const",
            SymbolKind::Module => "module",
            SymbolKind::Enum => "enum",
        }
    }
}

// ---------------------------------------------------------------------------
// Extracted data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ExtractedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub signature: String,
    pub body: String,
    pub start_line: usize,
    pub end_line: usize,
    pub children: Vec<ExtractedSymbol>,
}

#[derive(Debug, Clone)]
pub struct ExtractedReference {
    pub from_symbol: String,
    pub to_name: String,
    pub kind: String, // "calls", "type_ref", "imports"
}

// ---------------------------------------------------------------------------
// Grammar trait
// ---------------------------------------------------------------------------

pub trait Grammar: Send + Sync {
    fn language(&self) -> tree_sitter::Language;
    fn file_extensions(&self) -> &[&str];
    fn extract_symbols(&self, source: &[u8], tree: &tree_sitter::Tree) -> Vec<ExtractedSymbol>;
    fn extract_references(
        &self,
        source: &[u8],
        tree: &tree_sitter::Tree,
    ) -> Vec<ExtractedReference>;
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

pub struct GrammarRegistry {
    grammars: Vec<Box<dyn Grammar>>,
}

impl GrammarRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            grammars: Vec::new(),
        };
        registry.register(Box::new(go::GoGrammar));
        registry.register(Box::new(rust_lang::RustGrammar));
        registry.register(Box::new(typescript::TypeScriptGrammar));
        registry.register(Box::new(typescript::TsxGrammar));
        registry.register(Box::new(python::PythonGrammar));
        registry
    }

    pub fn register(&mut self, grammar: Box<dyn Grammar>) {
        self.grammars.push(grammar);
    }

    /// Look up the grammar that handles a given file extension (without the dot).
    pub fn for_extension(&self, ext: &str) -> Option<&dyn Grammar> {
        self.grammars
            .iter()
            .find(|g| g.file_extensions().contains(&ext))
            .map(|g| g.as_ref())
    }

    /// Detect the language name for a file path based on its extension.
    pub fn detect_language(&self, path: &Path) -> Option<&str> {
        let ext = path.extension()?.to_str()?;
        let grammar = self.for_extension(ext)?;
        // Return the first extension as the canonical language name.
        Some(grammar.file_extensions()[0])
    }
}

impl Default for GrammarRegistry {
    fn default() -> Self {
        Self::new()
    }
}
