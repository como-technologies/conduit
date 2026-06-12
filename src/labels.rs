//! Namespace-scoped label convergence (ADR-0007) — THE shared normalization
//! layer. conduit owns exactly the prefixes in [`OWNED_PREFIXES`]; a label
//! write replaces the owned subset of an object's labels (add missing,
//! remove stale) and passes every unprefixed human label through verbatim.
//! Both label-write paths (router and transcript) converge through
//! [`converge`]; the conformance suite proves the composed semantics on
//! every adapter.

/// The label namespaces conduit owns — frozen by ADR-0007; widening this
/// list requires a superseding decision.
pub const OWNED_PREFIXES: [&str; 3] = ["effort:", "adr:", "conduit:"];

/// True when conduit owns this label (it lives in an owned namespace).
pub fn is_owned(label: &str) -> bool {
    OWNED_PREFIXES.iter().any(|p| label.starts_with(p))
}

/// The convergent absolute label set: `current` labels OUTSIDE the owned
/// namespaces (preserved verbatim, in their current order) followed by
/// `desired_owned` (the machine state's owned set, in the given order).
/// Stale owned labels in `current` are dropped; duplicates collapse.
///
/// `desired_owned` must contain only owned labels — a non-owned desired
/// label is a programming error (debug-asserted), because it would let the
/// machine state overwrite human property.
pub fn converge(current: &[String], desired_owned: &[String]) -> Vec<String> {
    debug_assert!(
        desired_owned.iter().all(|l| is_owned(l)),
        "desired label set contains a non-owned label: {desired_owned:?}"
    );
    let mut out: Vec<String> = Vec::with_capacity(current.len() + desired_owned.len());
    for label in current {
        if !is_owned(label) && !out.contains(label) {
            out.push(label.clone());
        }
    }
    for label in desired_owned {
        if !out.contains(label) {
            out.push(label.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn owned_prefixes_are_the_adr_0007_trio() {
        assert_eq!(OWNED_PREFIXES, ["effort:", "adr:", "conduit:"]);
        assert!(is_owned("effort:1-super-quick"));
        assert!(is_owned("adr:ADR-0003"));
        assert!(is_owned("conduit:run"));
        assert!(!is_owned("priority-high"));
        assert!(!is_owned("conformance:x"), "unknown prefixes are human");
        assert!(!is_owned("Conduit:run"), "ownership is case-exact");
    }

    #[test]
    fn unprefixed_human_labels_survive_in_order() {
        let got = converge(
            &s(&["discuss", "adr:ADR-0003", "priority-high", "conduit:run"]),
            &s(&["conduit:failed"]),
        );
        assert_eq!(got, s(&["discuss", "priority-high", "conduit:failed"]));
    }

    #[test]
    fn stale_owned_labels_are_removed_and_missing_added() {
        let got = converge(
            &s(&["effort:1-super-quick", "adr:ADR-0003"]),
            &s(&["effort:3-average", "adr:ADR-0003"]),
        );
        assert_eq!(got, s(&["effort:3-average", "adr:ADR-0003"]));
    }

    #[test]
    fn empty_current_yields_exactly_the_desired_owned_set() {
        let got = converge(&[], &s(&["effort:1-super-quick", "adr:ADR-0003"]));
        assert_eq!(got, s(&["effort:1-super-quick", "adr:ADR-0003"]));
    }

    #[test]
    fn empty_desired_strips_all_owned_and_keeps_humans() {
        let got = converge(&s(&["conduit:failed", "keep-me"]), &[]);
        assert_eq!(got, s(&["keep-me"]));
    }

    #[test]
    fn convergence_is_idempotent() {
        let current = s(&["discuss", "effort:1-super-quick"]);
        let desired = s(&["effort:2-not-long", "adr:ADR-0001"]);
        let once = converge(&current, &desired);
        let twice = converge(&once, &desired);
        assert_eq!(once, twice, "fixed point after one application");
    }

    #[test]
    fn duplicates_collapse() {
        let got = converge(
            &s(&["discuss", "discuss"]),
            &s(&["adr:ADR-0001", "adr:ADR-0001"]),
        );
        assert_eq!(got, s(&["discuss", "adr:ADR-0001"]));
    }
}
