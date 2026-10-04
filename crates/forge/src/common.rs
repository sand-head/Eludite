//! Pieces the families share: paging through `Link` headers with a filter, user names, labels.

use serde_json::Value;

use crate::client::Client;
use crate::error::Result;
use crate::model::Page;

/// Pages read at most for one list call (a filter the forge cannot apply may need several).
pub const MAX_PAGES: usize = 10;

/// Read pages from `first` (or `cursor`, a previous answer's next url) until `max` items pass `keep` or the pages
/// end; the answer's `next_cursor` is the next page's url. A page is read whole, so a page's matches past `max` are
/// dropped and the next page continues after it.
pub fn collect<T>(
    client: &Client,
    first: String,
    cursor: Option<&str>,
    max: usize,
    parse: impl Fn(&Value) -> Option<T>,
    keep: impl Fn(&T) -> bool,
) -> Result<Page<T>> {
    let mut items = Vec::new();
    let mut next = Some(cursor.map(str::to_owned).unwrap_or(first));
    let mut pages = 0;
    while let Some(url) = next.take() {
        client.cancel.check()?;
        let (v, link) = client.get_page(&url)?;
        pages += 1;
        if let Value::Array(a) = v {
            for x in &a {
                if let Some(t) = parse(x)
                    && keep(&t)
                {
                    items.push(t);
                }
            }
        }
        next = link;
        if items.len() >= max || pages >= MAX_PAGES {
            break;
        }
    }
    items.truncate(max);
    Ok(Page {
        items,
        next_cursor: next,
    })
}

/// `user.login` style names from an array of users.
pub fn logins(v: Option<&Value>, key: &str) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|u| u.get(key).and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Label names from an array of label objects (or strings).
pub fn label_names(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|l| match l {
                    Value::String(s) => Some(s.clone()),
                    o => o.get("name").and_then(Value::as_str).map(str::to_owned),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The page size to ask for: `max`, at most 100.
pub fn per_page(max: usize) -> usize {
    max.clamp(1, 100)
}

/// Group comments into threads by their reply-to link (GitHub's `in_reply_to_id`, Forgejo's review id and position):
/// `key` gives a comment's thread key.
pub fn group_threads<K: PartialEq + Clone>(
    comments: Vec<(K, crate::model::Comment, Value)>,
) -> Vec<(K, Vec<crate::model::Comment>, Value)> {
    let mut out: Vec<(K, Vec<crate::model::Comment>, Value)> = Vec::new();
    for (k, c, raw) in comments {
        match out.iter_mut().find(|(x, _, _)| *x == k) {
            Some((_, list, _)) => list.push(c),
            None => out.push((k, vec![c], raw)),
        }
    }
    out
}
