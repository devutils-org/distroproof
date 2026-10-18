//! Output of `ldconfig -p`.

/// (soname, path) pairs in output order.
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (left, path) = line.split_once(" => ")?;
            let soname = left.split_whitespace().next()?;
            Some((soname.to_string(), path.trim().to_string()))
        })
        .collect()
}
