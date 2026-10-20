use crate::failure::Failure;

pub(super) struct Column {
    pub name: String,
    pub declared_type: String,
    pub not_null: bool,
    pub default: Option<String>,
    pub pk: usize,
}

pub(super) struct Table {
    pub name: String,
    pub columns: Vec<String>,
    pub definitions: Vec<Column>,
    pub sql: String,
    pub without_rowid: bool,
    pub autoincrement: bool,
}

pub(super) struct Index {
    pub name: String,
    pub table: String,
    pub sql: String,
}

pub(super) fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[derive(Clone)]
enum Token {
    Word(String),
    Name(String),
    Text(String),
    Number(String),
    Symbol(char),
    End,
}

struct Parser<'a> {
    source: &'a str,
    work: &'a super::Work,
    offset: usize,
    next: Token,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, work: &'a super::Work) -> Result<Self, Failure> {
        if source.len() > 65536 {
            return Err(super::refused());
        }
        super::checked_text(source, work)?;
        let mut parser = Self {
            source,
            work,
            offset: 0,
            next: Token::End,
        };
        parser.advance()?;
        Ok(parser)
    }

    fn advance(&mut self) -> Result<Token, Failure> {
        let previous = std::mem::replace(&mut self.next, Token::End);
        let bytes = self.source.as_bytes();
        while self.offset < bytes.len() && bytes[self.offset].is_ascii_whitespace() {
            self.offset += 1;
        }
        if self.offset == bytes.len() {
            return Ok(previous);
        }
        let start = self.offset;
        let first = bytes[start];
        self.offset += 1;
        self.next = match first {
            b'\'' | b'"' | b'`' | b'[' => {
                let close = if first == b'[' { b']' } else { first };
                let mut result = String::new();
                let mut chunk = self.offset;
                loop {
                    if self.offset == bytes.len() {
                        return Err(super::refused());
                    }
                    if bytes[self.offset] != close {
                        self.offset += 1;
                        continue;
                    }
                    result.push_str(&self.source[chunk..self.offset]);
                    self.offset += 1;
                    if first != b'[' && self.offset < bytes.len() && bytes[self.offset] == close {
                        result.push(close as char);
                        self.offset += 1;
                        chunk = self.offset;
                        continue;
                    }
                    break;
                }
                if first == b'\'' {
                    Token::Text(result)
                } else {
                    Token::Name(result)
                }
            }
            b'0'..=b'9' | b'.' => {
                while self.offset < bytes.len()
                    && (bytes[self.offset].is_ascii_digit()
                        || matches!(bytes[self.offset], b'.' | b'e' | b'E')
                        || (matches!(bytes[self.offset], b'+' | b'-')
                            && matches!(bytes[self.offset - 1], b'e' | b'E')))
                {
                    self.offset += 1;
                }
                Token::Number(self.source[start..self.offset].to_owned())
            }
            b'(' | b')' | b',' | b'+' | b'-' | b';' => Token::Symbol(first as char),
            _ if first.is_ascii_alphabetic() || first == b'_' || first >= 128 => {
                while self.offset < bytes.len()
                    && (bytes[self.offset].is_ascii_alphanumeric()
                        || bytes[self.offset] == b'_'
                        || bytes[self.offset] >= 128)
                {
                    self.offset += 1;
                }
                Token::Word(self.source[start..self.offset].to_owned())
            }
            _ => return Err(super::refused()),
        };
        Ok(previous)
    }

    fn keyword(&mut self, word: &str) -> Result<bool, Failure> {
        if matches!(&self.next, Token::Word(value) if value.eq_ignore_ascii_case(word)) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn require(&mut self, word: &str) -> Result<(), Failure> {
        if self.keyword(word)? {
            Ok(())
        } else {
            Err(super::refused())
        }
    }

    fn symbol(&mut self, symbol: char) -> Result<bool, Failure> {
        if matches!(self.next, Token::Symbol(value) if value == symbol) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn punctuation(&mut self, symbol: char) -> Result<(), Failure> {
        if self.symbol(symbol)? {
            Ok(())
        } else {
            Err(super::refused())
        }
    }

    fn name(&mut self) -> Result<String, Failure> {
        let value = match self.advance()? {
            Token::Word(value) | Token::Name(value) => value,
            _ => return Err(super::refused()),
        };
        if value.is_empty() || value.len() > 128 || value.contains('\0') {
            return Err(super::refused());
        }
        super::checked_text(&value, self.work)?;
        Ok(value)
    }

    fn names(&mut self) -> Result<Vec<String>, Failure> {
        self.punctuation('(')?;
        let mut names = Vec::new();
        loop {
            if names.len() == 32 {
                return Err(super::refused());
            }
            let name = self.name()?;
            if names
                .iter()
                .any(|prior: &String| prior.eq_ignore_ascii_case(&name))
            {
                return Err(super::refused());
            }
            names.push(name);
            if !self.symbol(',')? {
                break;
            }
        }
        self.punctuation(')')?;
        Ok(names)
    }

    fn column_type(&mut self) -> Result<String, Failure> {
        let name = match self.advance()? {
            Token::Word(value) => value.to_ascii_uppercase(),
            _ => return Err(super::refused()),
        };
        if !matches!(
            name.as_str(),
            "INTEGER"
                | "INT"
                | "REAL"
                | "FLOAT"
                | "DOUBLE"
                | "TEXT"
                | "VARCHAR"
                | "CHAR"
                | "NUMERIC"
                | "DECIMAL"
                | "BOOLEAN"
                | "BIGINT"
                | "SMALLINT"
        ) {
            return Err(super::refused());
        }
        let mut result = name.clone();
        if self.symbol('(')? {
            if !matches!(name.as_str(), "VARCHAR" | "CHAR" | "NUMERIC" | "DECIMAL") {
                return Err(super::refused());
            }
            let first = self.parameter()?;
            result.push_str(&format!("({first}"));
            if self.symbol(',')? {
                if !matches!(name.as_str(), "NUMERIC" | "DECIMAL") {
                    return Err(super::refused());
                }
                let second = self.parameter()?;
                if second > first {
                    return Err(super::refused());
                }
                result.push_str(&format!(",{second}"));
            }
            self.punctuation(')')?;
            result.push(')');
        }
        Ok(result)
    }

    fn parameter(&mut self) -> Result<u16, Failure> {
        match self.advance()? {
            Token::Number(value) if value.bytes().all(|byte| byte.is_ascii_digit()) => {
                value.parse().map_err(|_| super::refused())
            }
            _ => Err(super::refused()),
        }
    }

    fn literal(&mut self) -> Result<String, Failure> {
        let parenthesized = self.symbol('(')?;
        let sign = if self.symbol('-')? {
            "-"
        } else if self.symbol('+')? {
            "+"
        } else {
            ""
        };
        let literal = match self.advance()? {
            Token::Text(value) if sign.is_empty() => {
                super::checked_text(&value, self.work)?;
                format!("'{}'", value.replace('\'', "''"))
            }
            Token::Word(value) if sign.is_empty() && value.eq_ignore_ascii_case("NULL") => {
                "NULL".to_owned()
            }
            Token::Number(value) => {
                let number = format!("{sign}{value}");
                if !number
                    .parse::<f64>()
                    .map_err(|_| super::refused())?
                    .is_finite()
                {
                    return Err(super::refused());
                }
                super::checked_text(&number, self.work)?;
                number
            }
            _ => return Err(super::refused()),
        };
        if parenthesized {
            self.punctuation(')')?;
        }
        Ok(literal)
    }

    fn reference(&mut self) -> Result<String, Failure> {
        let table = self.name()?;
        let names = self.names()?;
        let mut result = format!("REFERENCES {}({})", quote(&table), quoted(&names));
        let mut delete = false;
        let mut update = false;
        while self.keyword("ON")? {
            let action = if self.keyword("DELETE")? && !delete {
                delete = true;
                "DELETE"
            } else if self.keyword("UPDATE")? && !update {
                update = true;
                "UPDATE"
            } else {
                return Err(super::refused());
            };
            let behavior = if self.keyword("NO")? {
                self.require("ACTION")?;
                "NO ACTION"
            } else if self.keyword("RESTRICT")? {
                "RESTRICT"
            } else if self.keyword("CASCADE")? {
                "CASCADE"
            } else if self.keyword("SET")? {
                if self.keyword("NULL")? {
                    "SET NULL"
                } else {
                    self.require("DEFAULT")?;
                    "SET DEFAULT"
                }
            } else {
                return Err(super::refused());
            };
            result.push_str(&format!(" ON {action} {behavior}"));
        }
        Ok(result)
    }

    fn finish(&mut self) -> Result<(), Failure> {
        self.symbol(';')?;
        if matches!(self.next, Token::End) {
            Ok(())
        } else {
            Err(super::refused())
        }
    }
}

fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|name| quote(name))
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn default_literal_with_work(sql: &str, work: &super::Work) -> Result<String, Failure> {
    let mut parser = Parser::new(sql, work)?;
    let literal = parser.literal()?;
    parser.finish()?;
    Ok(literal)
}

pub(super) fn type_name_with_work(sql: &str, work: &super::Work) -> Result<String, Failure> {
    let mut parser = Parser::new(sql, work)?;
    let name = parser.column_type()?;
    parser.finish()?;
    Ok(name)
}

#[cfg(test)]
pub(super) fn table(sql: &str) -> Result<Table, Failure> {
    table_with_work(sql, &super::Work::new(super::MAX_STEPS))
}
pub(super) fn table_with_work(sql: &str, work: &super::Work) -> Result<Table, Failure> {
    let mut parser = Parser::new(sql, work)?;
    parser.require("CREATE")?;
    parser.require("TABLE")?;
    let name = parser.name()?;
    if name.to_ascii_lowercase().starts_with("sqlite_") {
        return Err(super::refused());
    }
    parser.punctuation('(')?;
    let mut definitions: Vec<Column> = Vec::new();
    let mut fragments = Vec::new();
    let mut primary = Vec::new();
    let mut autoincrement = false;
    let mut constraints_started = false;
    loop {
        if parser.keyword("PRIMARY")? {
            constraints_started = true;
            parser.require("KEY")?;
            if !primary.is_empty() {
                return Err(super::refused());
            }
            primary = parser.names()?;
            fragments.push(format!("PRIMARY KEY({})", quoted(&primary)));
        } else if parser.keyword("UNIQUE")? {
            constraints_started = true;
            fragments.push(format!("UNIQUE({})", quoted(&parser.names()?)));
        } else if parser.keyword("FOREIGN")? {
            constraints_started = true;
            parser.require("KEY")?;
            let names = parser.names()?;
            parser.require("REFERENCES")?;
            fragments.push(format!(
                "FOREIGN KEY({}) {}",
                quoted(&names),
                parser.reference()?
            ));
        } else {
            if constraints_started || definitions.len() == 32 {
                return Err(super::refused());
            }
            let column_name = parser.name()?;
            if definitions
                .iter()
                .any(|column| column.name.eq_ignore_ascii_case(&column_name))
            {
                return Err(super::refused());
            }
            let declared_type = parser.column_type()?;
            let mut fragment = format!("{} {declared_type}", quote(&column_name));
            let mut not_null = false;
            let mut default = None;
            let mut unique = false;
            let mut reference = false;
            loop {
                if parser.keyword("PRIMARY")? {
                    if !primary.is_empty() {
                        return Err(super::refused());
                    }
                    parser.require("KEY")?;
                    primary.push(column_name.clone());
                    fragment.push_str(" PRIMARY KEY");
                    if parser.keyword("AUTOINCREMENT")? {
                        if declared_type != "INTEGER" {
                            return Err(super::refused());
                        }
                        autoincrement = true;
                        fragment.push_str(" AUTOINCREMENT");
                    }
                } else if parser.keyword("NOT")? {
                    if not_null {
                        return Err(super::refused());
                    }
                    parser.require("NULL")?;
                    not_null = true;
                    fragment.push_str(" NOT NULL");
                } else if parser.keyword("DEFAULT")? {
                    if default.is_some() {
                        return Err(super::refused());
                    }
                    let value = parser.literal()?;
                    fragment.push_str(&format!(" DEFAULT {value}"));
                    default = Some(value);
                } else if parser.keyword("UNIQUE")? {
                    if unique {
                        return Err(super::refused());
                    }
                    unique = true;
                    fragment.push_str(" UNIQUE");
                } else if parser.keyword("REFERENCES")? {
                    if reference {
                        return Err(super::refused());
                    }
                    reference = true;
                    fragment.push_str(&format!(" {}", parser.reference()?));
                } else {
                    break;
                }
            }
            definitions.push(Column {
                name: column_name,
                declared_type,
                not_null,
                default,
                pk: 0,
            });
            fragments.push(fragment);
        }
        if !parser.symbol(',')? {
            break;
        }
    }
    parser.punctuation(')')?;
    let without_rowid = parser.keyword("WITHOUT")?;
    if without_rowid {
        parser.require("ROWID")?;
    }
    parser.finish()?;
    if definitions.is_empty() || primary.is_empty() || (without_rowid && autoincrement) {
        return Err(super::refused());
    }
    for (position, key) in primary.iter().enumerate() {
        let column = definitions
            .iter_mut()
            .find(|column| column.name.eq_ignore_ascii_case(key))
            .ok_or_else(super::refused)?;
        column.pk = position + 1;
    }
    if !without_rowid
        && (primary.len() != 1
            || definitions
                .iter()
                .find(|column| column.pk == 1)
                .is_none_or(|column| column.declared_type != "INTEGER"))
    {
        return Err(super::refused());
    }
    let sql = format!(
        "CREATE TABLE {}({}){}",
        quote(&name),
        fragments.join(","),
        if without_rowid { " WITHOUT ROWID" } else { "" }
    );
    let columns = definitions
        .iter()
        .map(|column| column.name.clone())
        .collect();
    Ok(Table {
        name,
        columns,
        definitions,
        sql,
        without_rowid,
        autoincrement,
    })
}

#[cfg(test)]
pub(super) fn index(sql: &str) -> Result<Index, Failure> {
    index_with_work(sql, &super::Work::new(super::MAX_STEPS))
}
pub(super) fn index_with_work(sql: &str, work: &super::Work) -> Result<Index, Failure> {
    let mut parser = Parser::new(sql, work)?;
    parser.require("CREATE")?;
    let unique = parser.keyword("UNIQUE")?;
    parser.require("INDEX")?;
    let name = parser.name()?;
    if name.to_ascii_lowercase().starts_with("sqlite_") {
        return Err(super::refused());
    }
    parser.require("ON")?;
    let table = parser.name()?;
    parser.punctuation('(')?;
    let mut columns = Vec::new();
    loop {
        if columns.len() == 32 {
            return Err(super::refused());
        }
        let name = parser.name()?;
        let order = if parser.keyword("ASC")? {
            " ASC"
        } else if parser.keyword("DESC")? {
            " DESC"
        } else {
            ""
        };
        columns.push(format!("{}{order}", quote(&name)));
        if !parser.symbol(',')? {
            break;
        }
    }
    parser.punctuation(')')?;
    parser.finish()?;
    let sql = format!(
        "CREATE {}INDEX {} ON {}({})",
        if unique { "UNIQUE " } else { "" },
        quote(&name),
        quote(&table),
        columns.join(",")
    );
    Ok(Index { name, table, sql })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_schema_accepts_confirmed_profile_and_generates_quoted_ddl() {
        let table = table("CREATE TABLE item(id INTEGER PRIMARY KEY AUTOINCREMENT, label VARCHAR(80) NOT NULL DEFAULT ('ready') UNIQUE, parent INTEGER REFERENCES item(id) ON DELETE SET NULL)").unwrap();
        assert_eq!(table.name, "item");
        assert_eq!(table.columns, ["id", "label", "parent"]);
        assert!(table.autoincrement);
        assert!(!table.without_rowid);
        assert!(table.sql.starts_with("CREATE TABLE \"item\""));
        let table = super::table(
            "CREATE TABLE pair(a TEXT,b DECIMAL(12,2),PRIMARY KEY(a,b)) WITHOUT ROWID",
        )
        .unwrap();
        assert!(table.without_rowid);
        let index = index("CREATE UNIQUE INDEX ordered ON item(label DESC,id ASC)").unwrap();
        assert_eq!(index.name, "ordered");
        assert_eq!(index.table, "item");
        assert!(index.sql.starts_with("CREATE UNIQUE INDEX \"ordered\""));
    }

    #[test]
    fn strict_schema_refuses_every_unknown_clause_and_sensitive_literal() {
        for sql in [
            "CREATE TABLE item(id INTEGER PRIMARY KEY DESC)",
            "CREATE TABLE item(id INTEGER PRIMARY KEY, value TEXT COLLATE NOCASE)",
            "CREATE TABLE item(id INTEGER PRIMARY KEY, value TEXT CHECK(length(value)>0))",
            "CREATE TABLE item(id INTEGER PRIMARY KEY, value TEXT DEFAULT (lower('ready')))",
            "CREATE TABLE item(id INTEGER PRIMARY KEY, value TEXT DEFAULT 'person@example.com')",
            "CREATE TABLE item(id INTEGER PRIMARY KEY, CONSTRAINT named UNIQUE(id))",
            "CREATE TABLE item(id INTEGER PRIMARY KEY ON CONFLICT IGNORE)",
            "CREATE TABLE item(id INTEGER PRIMARY KEY, value DECIMAL(2,3))",
            "CREATE TABLE item(id INTEGER PRIMARY KEY, value TEXT REFERENCES item(id) DEFERRABLE)",
            "CREATE TABLE item(id INTEGER PRIMARY KEY) STRICT",
            "CREATE TABLE item(id INTEGER PRIMARY KEY); DROP TABLE item",
        ] {
            assert!(table(sql).is_err(), "Unsupported schema must refuse.");
        }
        for sql in [
            "CREATE INDEX expr ON item(lower(value))",
            "CREATE INDEX partial ON item(value) WHERE id>0",
            "CREATE INDEX collated ON item(value COLLATE NOCASE)",
        ] {
            assert!(index(sql).is_err());
        }
    }
}
