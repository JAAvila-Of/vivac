use crate::failure::Failure;

pub(super) struct Table {
    pub name: String,
    pub columns: Vec<String>,
    pub sql: String,
    pub without_rowid: bool,
    pub autoincrement: bool,
}

pub(super) struct Index {
    pub name: String,
    pub table: String,
    pub sql: String,
}

pub(super) fn table(_sql: &str) -> Result<Table, Failure> {
    Err(super::refused())
}

pub(super) fn index(_sql: &str) -> Result<Index, Failure> {
    Err(super::refused())
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
