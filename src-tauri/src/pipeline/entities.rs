//! Entity taxonomy classification (fast tier, local, table-driven).
//! Order matters: first match wins, so more specific patterns come first.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entity {
    Color,
    Email,
    Phone,
    Ip,
    Uuid,
    Coordinates,
    Currency,
    Date,
    FilePath,
    Url,
    Code(CodeLang),
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeLang {
    Rust,
    Js,
    Python,
    Sql,
    Shell,
    Unknown,
}

impl Entity {
    /// Stable string for the `captures.entity_type` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Entity::Color => "color",
            Entity::Email => "email",
            Entity::Phone => "phone",
            Entity::Ip => "ip",
            Entity::Uuid => "uuid",
            Entity::Coordinates => "coordinates",
            Entity::Currency => "currency",
            Entity::Date => "date",
            Entity::FilePath => "filepath",
            Entity::Url => "url",
            Entity::Code(CodeLang::Rust) => "code:rust",
            Entity::Code(CodeLang::Js) => "code:js",
            Entity::Code(CodeLang::Python) => "code:python",
            Entity::Code(CodeLang::Sql) => "code:sql",
            Entity::Code(CodeLang::Shell) => "code:shell",
            Entity::Code(CodeLang::Unknown) => "code",
            Entity::Text => "text",
        }
    }

    /// Human phrase used to build the type-enriched embedding input (FR9a).
    pub fn enrich_prefix(&self) -> &'static str {
        match self {
            Entity::Color => "hex color code",
            Entity::Email => "email address",
            Entity::Phone => "phone number",
            Entity::Ip => "ip address",
            Entity::Uuid => "uuid",
            Entity::Coordinates => "geographic coordinates",
            Entity::Currency => "currency amount",
            Entity::Date => "date",
            Entity::FilePath => "file path",
            Entity::Url => "url",
            Entity::Code(CodeLang::Rust) => "rust code",
            Entity::Code(CodeLang::Js) => "javascript code",
            Entity::Code(CodeLang::Python) => "python code",
            Entity::Code(CodeLang::Sql) => "sql code",
            Entity::Code(CodeLang::Shell) => "shell command",
            Entity::Code(CodeLang::Unknown) => "code",
            Entity::Text => "text",
        }
    }
}

struct Rule {
    entity: Entity,
    // whole-string match against the trimmed capture
    pattern: &'static str,
}

// Applied to the *whole trimmed text*; these only classify short single-value
// captures. Longer text falls through to code/text detection.
const VALUE_RULES: &[Rule] = &[
    Rule { entity: Entity::Color, pattern: r"^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$|^rgba?\(\s*\d{1,3}\s*,\s*\d{1,3}\s*,\s*\d{1,3}\s*(?:,\s*[\d.]+\s*)?\)$" },
    Rule { entity: Entity::Uuid, pattern: r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$" },
    Rule { entity: Entity::Email, pattern: r"^[\w.+-]+@[\w-]+\.[\w.-]+$" },
    Rule { entity: Entity::Url, pattern: r"^(?:https?|ftp)://\S+$" },
    Rule { entity: Entity::Ip, pattern: r"^(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?$|^(?:[0-9a-fA-F]{0,4}:){2,7}[0-9a-fA-F]{0,4}$" },
    // date / coordinates / currency must precede phone: its charset ([\d\s().-])
    // would swallow ISO dates and similar numeric shapes
    Rule { entity: Entity::Date, pattern: r"^\d{4}-\d{2}-\d{2}(?:[T ][\d:.]+Z?)?$|^\d{1,2}[/-]\d{1,2}[/-]\d{2,4}$" },
    Rule { entity: Entity::Coordinates, pattern: r"^-?\d{1,3}\.\d+\s*,\s*-?\d{1,3}\.\d+$" },
    Rule { entity: Entity::Currency, pattern: r"^[$€£₹¥]\s?[\d,]+(?:\.\d+)?$|^[\d,]+(?:\.\d+)?\s?(?:USD|EUR|GBP|INR|JPY)$" },
    Rule { entity: Entity::Phone, pattern: r"^\+?[\d\s().-]{7,20}$" },
    Rule { entity: Entity::FilePath, pattern: r"^(?:[A-Za-z]:[\\/]|\\\\|/|~/)[^\r\n]*$" },
];

fn compiled() -> &'static Vec<(Entity, Regex)> {
    static RULES: OnceLock<Vec<(Entity, Regex)>> = OnceLock::new();
    RULES.get_or_init(|| {
        VALUE_RULES
            .iter()
            .map(|r| (r.entity, Regex::new(r.pattern).expect("valid rule regex")))
            .collect()
    })
}

pub fn classify(text: &str) -> Entity {
    let t = text.trim();
    if t.is_empty() {
        return Entity::Text;
    }
    // single-value rules only apply to single-line, shortish captures
    if t.len() <= 200 && !t.contains('\n') {
        for (entity, re) in compiled() {
            if re.is_match(t) {
                // phone is a greedy pattern; don't let plain numbers-with-spaces
                // that are clearly amounts/dates slip in — digits required
                if *entity == Entity::Phone && t.chars().filter(|c| c.is_ascii_digit()).count() < 7 {
                    continue;
                }
                return *entity;
            }
        }
    }
    detect_code(t).map(Entity::Code).unwrap_or(Entity::Text)
}

/// Cheap syntax-density heuristic — good enough for the fast tier.
fn detect_code(t: &str) -> Option<CodeLang> {
    let signals = [
        ("fn ", 3), ("let ", 2), ("impl ", 3), ("::", 2), ("->", 2), ("=>", 2),
        ("function ", 3), ("const ", 2), ("var ", 1), ("import ", 2), ("return ", 2),
        ("def ", 3), ("class ", 2), ("SELECT ", 4), ("INSERT ", 4), ("CREATE ", 3),
        ("{", 1), ("}", 1), (";", 2), ("()", 2), ("#!/", 4), ("$ ", 1),
    ];
    let score: i32 = signals
        .iter()
        .map(|(s, w)| (t.matches(s).count().min(3) as i32) * w)
        .sum();
    let threshold = if t.contains('\n') { 4 } else { 5 };
    if score < threshold {
        return None;
    }
    let lang = if t.contains("fn ") && (t.contains("::") || t.contains("->")) {
        CodeLang::Rust
    } else if t.contains("def ") || (t.contains("import ") && !t.contains('{')) {
        CodeLang::Python
    } else if t.to_uppercase().contains("SELECT ") || t.to_uppercase().contains("CREATE TABLE") {
        CodeLang::Sql
    } else if t.starts_with("#!") || t.starts_with("$ ") {
        CodeLang::Shell
    } else if t.contains("const ") || t.contains("function ") || t.contains("=>") {
        CodeLang::Js
    } else {
        CodeLang::Unknown
    };
    Some(lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_entities() {
        let cases: &[(&str, &str)] = &[
            ("#3B82F6", "color"),
            ("rgb(16, 185, 129)", "color"),
            ("alice@example.com", "email"),
            ("+91 98765 43210", "phone"),
            ("192.168.1.1", "ip"),
            ("550e8400-e29b-41d4-a716-446655440000", "uuid"),
            ("28.6139, 77.2090", "coordinates"),
            ("$1,299.99", "currency"),
            ("2026-07-03", "date"),
            (r"C:\Users\adamy\Desktop", "filepath"),
            ("/usr/local/bin", "filepath"),
            ("https://github.com/asg017/sqlite-vec", "url"),
            ("just a normal sentence about things", "text"),
        ];
        for (input, expected) in cases {
            assert_eq!(classify(input).as_str(), *expected, "input: {input}");
        }
    }

    #[test]
    fn code_detection() {
        assert_eq!(
            classify("fn main() -> Result<()> {\n    let x = foo::bar();\n}").as_str(),
            "code:rust"
        );
        assert_eq!(
            classify("const store = useAuthStore();\nstore.login(user);").as_str(),
            "code:js"
        );
        assert_eq!(
            classify("SELECT * FROM captures WHERE id = 1;").as_str(),
            "code:sql"
        );
        assert_eq!(classify("hello world, nothing here").as_str(), "text");
    }
}
