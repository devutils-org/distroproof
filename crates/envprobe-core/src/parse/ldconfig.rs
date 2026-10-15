//! Sortie de `ldconfig -p`.

/// Paires (soname, chemin) dans l'ordre de la sortie.
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (left, path) = line.split_once(" => ")?;
            let soname = left.split_whitespace().next()?;
            Some((soname.to_string(), path.trim().to_string()))
        })
        .collect()
}
