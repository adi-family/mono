use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum ConfigValue {
    Null,
    Str(String),
    Num(f64),
    Bool(bool),
    List(Vec<ConfigValue>),
    Map(HashMap<String, ConfigValue>),
}

impl ConfigValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ConfigValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ConfigValue::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            ConfigValue::Num(n) if *n >= 0.0 && *n == (*n as u64) as f64 => Some(*n as u64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConfigValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[ConfigValue]> {
        match self {
            ConfigValue::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&HashMap<String, ConfigValue>> {
        match self {
            ConfigValue::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&ConfigValue> {
        match self {
            ConfigValue::Map(m) => m.get(key),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, ConfigValue::Null)
    }

    pub fn from_pairs(pairs: &[(&str, ConfigValue)]) -> ConfigValue {
        ConfigValue::Map(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }
}

impl Default for ConfigValue {
    fn default() -> Self {
        ConfigValue::Map(HashMap::new())
    }
}

impl From<&str> for ConfigValue {
    fn from(s: &str) -> Self {
        ConfigValue::Str(s.to_string())
    }
}

impl From<String> for ConfigValue {
    fn from(s: String) -> Self {
        ConfigValue::Str(s)
    }
}

impl From<f64> for ConfigValue {
    fn from(n: f64) -> Self {
        ConfigValue::Num(n)
    }
}

impl From<bool> for ConfigValue {
    fn from(b: bool) -> Self {
        ConfigValue::Bool(b)
    }
}

impl ConfigValue {
    pub fn to_json(&self) -> String {
        match self {
            ConfigValue::Null => "null".to_string(),
            ConfigValue::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            ConfigValue::Num(n) => {
                if *n == (*n as i64) as f64 {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            ConfigValue::Str(s) => {
                let mut out = String::with_capacity(s.len() + 2);
                out.push('"');
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if c < '\x20' => out.push_str(&format!("\\u{:04x}", c as u32)),
                        c => out.push(c),
                    }
                }
                out.push('"');
                out
            }
            ConfigValue::List(items) => {
                let mut out = String::from("[");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&item.to_json());
                }
                out.push(']');
                out
            }
            ConfigValue::Map(map) => {
                let mut out = String::from("{");
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for (i, key) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&ConfigValue::Str((*key).clone()).to_json());
                    out.push(':');
                    out.push_str(&map[*key].to_json());
                }
                out.push('}');
                out
            }
        }
    }

    pub fn from_json(input: &str) -> Result<Self, String> {
        let bytes = input.as_bytes();
        let (val, pos) = parse_value(bytes, skip_ws(bytes, 0))?;
        let pos = skip_ws(bytes, pos);
        if pos != bytes.len() {
            return Err(format!("trailing data at position {pos}"));
        }
        Ok(val)
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

fn parse_value(b: &[u8], i: usize) -> Result<(ConfigValue, usize), String> {
    if i >= b.len() {
        return Err("unexpected end of input".to_string());
    }
    match b[i] {
        b'"' => parse_string(b, i).map(|(s, pos)| (ConfigValue::Str(s), pos)),
        b'{' => parse_object(b, i),
        b'[' => parse_array(b, i),
        b't' => parse_literal(b, i, b"true", ConfigValue::Bool(true)),
        b'f' => parse_literal(b, i, b"false", ConfigValue::Bool(false)),
        b'n' => parse_literal(b, i, b"null", ConfigValue::Null),
        b'-' | b'0'..=b'9' => parse_number(b, i),
        c => Err(format!("unexpected char '{}' at {i}", c as char)),
    }
}

fn parse_literal(
    b: &[u8],
    i: usize,
    lit: &[u8],
    val: ConfigValue,
) -> Result<(ConfigValue, usize), String> {
    if b[i..].starts_with(lit) {
        Ok((val, i + lit.len()))
    } else {
        Err(format!("invalid literal at {i}"))
    }
}

fn parse_number(b: &[u8], mut i: usize) -> Result<(ConfigValue, usize), String> {
    let start = i;
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    let s = std::str::from_utf8(&b[start..i]).map_err(|e| e.to_string())?;
    let n: f64 = s
        .parse()
        .map_err(|e: std::num::ParseFloatError| e.to_string())?;
    Ok((ConfigValue::Num(n), i))
}

fn parse_string(b: &[u8], mut i: usize) -> Result<(String, usize), String> {
    i += 1; // skip opening "
    let mut s = String::new();
    while i < b.len() {
        match b[i] {
            b'"' => return Ok((s, i + 1)),
            b'\\' => {
                i += 1;
                if i >= b.len() {
                    return Err("unexpected end in string escape".to_string());
                }
                match b[i] {
                    b'"' => s.push('"'),
                    b'\\' => s.push('\\'),
                    b'/' => s.push('/'),
                    b'n' => s.push('\n'),
                    b'r' => s.push('\r'),
                    b't' => s.push('\t'),
                    b'b' => s.push('\u{0008}'),
                    b'f' => s.push('\u{000C}'),
                    b'u' => {
                        i += 1;
                        if i + 4 > b.len() {
                            return Err("incomplete \\u escape".to_string());
                        }
                        let hex = std::str::from_utf8(&b[i..i + 4]).map_err(|e| e.to_string())?;
                        let cp = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
                        if let Some(c) = char::from_u32(cp) {
                            s.push(c);
                        }
                        i += 3; // loop will +1
                    }
                    c => return Err(format!("unknown escape '\\{}'", c as char)),
                }
                i += 1;
            }
            _ => {
                // UTF-8 safe: find the char boundary
                let ch = std::str::from_utf8(&b[i..])
                    .map_err(|e| e.to_string())?
                    .chars()
                    .next()
                    .ok_or("empty char")?;
                s.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    Err("unterminated string".to_string())
}

fn parse_array(b: &[u8], mut i: usize) -> Result<(ConfigValue, usize), String> {
    i = skip_ws(b, i + 1); // skip [
    let mut items = Vec::new();
    if i < b.len() && b[i] == b']' {
        return Ok((ConfigValue::List(items), i + 1));
    }
    loop {
        let (val, pos) = parse_value(b, skip_ws(b, i))?;
        items.push(val);
        i = skip_ws(b, pos);
        if i >= b.len() {
            return Err("unterminated array".to_string());
        }
        match b[i] {
            b']' => return Ok((ConfigValue::List(items), i + 1)),
            b',' => i += 1,
            c => return Err(format!("expected ',' or ']', got '{}'", c as char)),
        }
    }
}

fn parse_object(b: &[u8], mut i: usize) -> Result<(ConfigValue, usize), String> {
    i = skip_ws(b, i + 1); // skip {
    let mut map = HashMap::new();
    if i < b.len() && b[i] == b'}' {
        return Ok((ConfigValue::Map(map), i + 1));
    }
    loop {
        i = skip_ws(b, i);
        if i >= b.len() || b[i] != b'"' {
            return Err(format!("expected string key at {i}"));
        }
        let (key, pos) = parse_string(b, i)?;
        i = skip_ws(b, pos);
        if i >= b.len() || b[i] != b':' {
            return Err(format!("expected ':' at {i}"));
        }
        i = skip_ws(b, i + 1);
        let (val, pos) = parse_value(b, i)?;
        map.insert(key, val);
        i = skip_ws(b, pos);
        if i >= b.len() {
            return Err("unterminated object".to_string());
        }
        match b[i] {
            b'}' => return Ok((ConfigValue::Map(map), i + 1)),
            b',' => i += 1,
            c => return Err(format!("expected ',' or '}}', got '{}'", c as char)),
        }
    }
}
