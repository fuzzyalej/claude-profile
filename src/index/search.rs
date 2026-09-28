use crate::index::model::IndexEntry;

/// True when `needle` occurs in `haystack` at the start of a word: a match
/// preceded by a separator (`-`, `_`, `.`, whitespace) or at position 0. A
/// match buried inside a longer word does not count, so `rails` hits
/// `ultra-rails-skills` but not `guardrails`. Both sides are already
/// lowercased by the caller.
fn word_starts_with(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    haystack
        .match_indices(needle)
        .any(|(i, _)| i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
}

fn term_score(name: &str, desc: &str, cat: &str, term: &str) -> u32 {
    let mut score = 0;
    if name == term {
        score += 6;
    } else if word_starts_with(name, term) {
        score += 3;
    }
    if word_starts_with(desc, term) {
        score += 2;
    }
    if word_starts_with(cat, term) {
        score += 1;
    }
    score
}

/// Total score across every query term, or 0 when any term matches nothing:
/// a multi-word query keeps only entries that hit all of its terms.
fn score(entry: &IndexEntry, terms: &[String]) -> u32 {
    let name = entry.plugin.to_lowercase();
    let desc = entry.description.to_lowercase();
    let cat = entry.category.as_deref().unwrap_or("").to_lowercase();
    let mut total = 0;
    for t in terms {
        match term_score(&name, &desc, &cat, t) {
            0 => return 0,
            s => total += s,
        }
    }
    total
}

pub fn rank<'a>(
    entries: &'a [IndexEntry],
    query: &str,
    marketplace: Option<&str>,
    limit: usize,
) -> Vec<&'a IndexEntry> {
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    let mkt = marketplace.map(|m| m.to_lowercase());
    let mut scored: Vec<(u32, &IndexEntry)> = entries
        .iter()
        .filter(|e| match &mkt {
            Some(m) => e.marketplace.to_lowercase() == *m,
            None => true,
        })
        .map(|e| (score(e, &terms), e))
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.plugin.cmp(&b.1.plugin)));
    scored.into_iter().take(limit).map(|(_, e)| e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(plugin: &str, mkt: &str, desc: &str, cat: Option<&str>) -> IndexEntry {
        IndexEntry {
            plugin: plugin.into(),
            marketplace: mkt.into(),
            repo: format!("owner/{mkt}"),
            description: desc.into(),
            category: cat.map(|c| c.into()),
        }
    }

    #[test]
    fn ranks_name_hits_above_description_hits() {
        let entries = vec![
            e("django-helper", "a", "web framework", None),
            e("logger", "a", "python logging utility", None),
            e("python", "a", "the python toolkit", None),
        ];
        let got = rank(&entries, "python", None, 10);
        assert_eq!(got[0].plugin, "python");     // exact name
        assert_eq!(got[1].plugin, "logger");     // description hit
        assert_eq!(got.len(), 2);                // django-helper scores 0, dropped
    }

    #[test]
    fn matches_only_at_word_starts() {
        let entries = vec![
            e("guardrails-x", "a", "audit with guardrails", None),
            e("ultra-rails-skills", "a", "hyphenated name", None),
            e("layered-rails", "a", "review Rails apps", None),
        ];
        let got = rank(&entries, "rails", None, 10);
        let names: Vec<&str> = got.iter().map(|e| e.plugin.as_str()).collect();
        assert_eq!(names, vec!["layered-rails", "ultra-rails-skills"]);
    }

    #[test]
    fn drops_entries_missing_any_term() {
        let entries = vec![
            e("both", "a", "backend architecture guide", None),
            e("one-only", "a", "backend only", None),
        ];
        let got = rank(&entries, "backend architecture", None, 10);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].plugin, "both");
    }

    #[test]
    fn multi_term_and_marketplace_filter_and_limit() {
        let entries = vec![
            e("a", "mkt1", "backend architecture guide", None),
            e("b", "mkt2", "backend only", None),
            e("c", "mkt1", "architecture only", None),
        ];
        let got = rank(&entries, "backend architecture", Some("mkt1"), 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].plugin, "a");          // both terms, in mkt1
    }
}
