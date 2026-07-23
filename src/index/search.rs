use crate::index::model::IndexEntry;

fn score(entry: &IndexEntry, terms: &[String]) -> u32 {
    let name = entry.plugin.to_lowercase();
    let desc = entry.description.to_lowercase();
    let cat = entry.category.as_deref().unwrap_or("").to_lowercase();
    let mut total = 0;
    for t in terms {
        if name == *t {
            total += 6;
        } else if name.contains(t.as_str()) {
            total += 3;
        }
        if desc.contains(t.as_str()) {
            total += 2;
        }
        if cat.contains(t.as_str()) {
            total += 1;
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
