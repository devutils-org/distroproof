//! Sondes d'environnement Linux.
//!
//! Chaque catégorie (identité, kernel, BPF…) sera un module exposant
//! `probe() -> Section`. Les sondes arrivent en phase 1.

/// Une catégorie du rapport, par exemple « kernel » ou « bpf ».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: &'static str,
    pub entries: Vec<(String, String)>,
}

/// Exécute toutes les sondes disponibles.
pub fn probe_all() -> Vec<Section> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_all_does_not_panic() {
        assert!(probe_all().is_empty());
    }
}
