//! The `LIMIT` a query of this adapter writes for the rows a port asks for.

/// The rows a query returns at most, as SurrealDB takes them: a `LIMIT` is a
/// count that fits a `u32` (4.0 refuses any other value), so a port's limit
/// beyond it asks for every row and the query has no `LIMIT`.
#[derive(Clone, Copy)]
pub struct Limit(Option<u32>);

impl Limit {
    /// At most `rows` rows.
    pub fn of(rows: usize) -> Self {
        match u32::try_from(rows) {
            Ok(rows) => Self(Some(rows)),
            Err(_) => Self(None),
        }
    }

    /// The clause, `LIMIT $limit` or nothing, to write where the query pages.
    pub fn clause(self) -> &'static str {
        match self.0 {
            Some(_) => "LIMIT $limit",
            None => "",
        }
    }

    /// The value of `$limit`: `NONE` when there is no clause to read it.
    pub fn rows(self) -> Option<u32> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limit_past_u32_asks_for_every_row() {
        assert_eq!(Limit::of(5).clause(), "LIMIT $limit");
        assert_eq!(Limit::of(5).rows(), Some(5));
        assert_eq!(Limit::of(u32::MAX as usize).rows(), Some(u32::MAX));
        assert_eq!(Limit::of(usize::MAX).clause(), "");
        assert_eq!(Limit::of(usize::MAX).rows(), None);
    }
}
