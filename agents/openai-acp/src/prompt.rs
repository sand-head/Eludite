//! The system prompt: who the agent is, where it works, the rules, and the IDE's guides read from its MCP
//! resources at session start (`eludite://guides/debugging`, `git`, `terminal`), each under its heading.
//!
//! The whole prompt stays under [`BUDGET_TOKENS`] by the bytes / 4 estimate (no tokenizer): the rules take about a
//! fifth of it, and a guide that does not fit what is left is cut at a paragraph with a line saying so.

use std::path::Path;

/// The prompt's budget in estimated tokens.
pub const BUDGET_TOKENS: usize = 4_000;

/// The guides appended, in order: (resource uri, heading).
pub const GUIDES: &[(&str, &str)] = &[
    ("eludite://guides/debugging", "Driving the debugger"),
    ("eludite://guides/git", "Using git"),
    (
        "eludite://guides/terminal",
        "Running commands in the terminal",
    ),
];

/// Tokens by the bytes / 4 estimate.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// `YYYY-MM-DD` (UTC) for seconds since the Unix epoch.
pub fn date_from_unix(secs: u64) -> String {
    // Howard Hinnant's civil-from-days.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Today's date (UTC).
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    date_from_unix(secs)
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "macOS",
        "windows" => "Windows",
        other => other,
    }
}

/// The rules, before the guides.
pub fn base_prompt(cwd: &Path, os: &str, date: &str, has_tools: bool) -> String {
    let mut s = format!(
        "You are a coding agent working inside Eludite, an IDE, on the workspace at {cwd} ({os}). Today is {date}.\n\
         \n\
         You act only through the IDE's tools. Each tool is an Eludite command, the same one the person runs from \
         the menus, with the same permissions: reads run at once; edits are held as pending changes the person \
         reviews; building, running tests, the terminal and other actions may ask the person first, and a refused \
         call comes back as the tool's error. You have no shell and no file access of your own.\n\
         \n\
         Rules:\n\
         - Use the tools to find things out. Never claim to have read, run, built or tested something you did not \
         do through a tool in this conversation, and quote what the tools returned rather than guessing.\n\
         - Read a file with eludite-file-read before editing it, and edit only text you have just read.\n\
         - For a small change use eludite-file-edit (oldText must occur exactly once; include enough surrounding \
         lines). For many changes across files use eludite-workspace-apply_edit with one workspace edit.\n\
         - To find code, prefer eludite-search-find over reading files one by one, then \
         eludite-editor-go_to_definition and eludite-editor-find_references.\n\
         - Build and run tests through the tools (eludite-build-*, eludite-test-run), then read the results \
         (diagnostics-list, eludite-test-results, eludite-output-show) before saying the work is done.\n\
         - Paths may be absolute or relative to the workspace root.\n\
         - Call one tool at a time when a call depends on an earlier result. Pass arguments as a JSON object that \
         matches the tool's schema.\n\
         - If a tool fails, read its error and change what you send; do not repeat the same call.\n\
         - Be brief. When you are done, say what changed and what you verified.\n",
        cwd = cwd.display()
    );
    if has_tools {
        s.push_str(
            "- You see the core tools. Call eludite-tools to list the others (with a prefix such as \
             eludite-debug- or eludite-git-) and enable: [names] to add them.\n",
        );
    } else {
        s.push_str("- No IDE tools are connected in this session, so you can only answer from the conversation.\n");
    }
    s
}

/// Cut `text` to at most `max` bytes at a paragraph (else a line, else a character) boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    let at = head
        .rfind("\n\n")
        .or_else(|| head.rfind('\n'))
        .unwrap_or(end);
    &text[..at]
}

/// The whole system prompt: the rules, then each guide `(uri, heading, text)` under its heading, within budget.
/// The room left after the rules is shared out smallest guide first, so short guides go in whole and a long one
/// (the debugging guide) is cut at a paragraph with a line saying where the rest is.
pub fn system_prompt(
    cwd: &Path,
    os: Option<&str>,
    date: Option<&str>,
    has_tools: bool,
    guides: &[(String, String, String)],
) -> String {
    let today = today();
    let mut s = base_prompt(
        cwd,
        os.unwrap_or(os_name()),
        date.unwrap_or(&today),
        has_tools,
    );
    let heads: Vec<String> = guides
        .iter()
        .map(|(_, heading, _)| format!("\n# {heading}\n\n"))
        .collect();
    let notes: Vec<String> = guides
        .iter()
        .map(|(uri, _, _)| format!("\n\n(The rest of {uri} is left out here.)\n"))
        .collect();
    let mut room = (BUDGET_TOKENS * 4)
        .saturating_sub(s.len() + heads.iter().map(String::len).sum::<usize>() + 16);
    // Water-filling: each guide, smallest first, gets at most an equal share of what is left.
    let mut order: Vec<usize> = (0..guides.len()).collect();
    order.sort_by_key(|&i| guides[i].2.trim().len());
    let mut alloc = vec![0usize; guides.len()];
    for (k, &i) in order.iter().enumerate() {
        let share = room / (order.len() - k);
        let len = guides[i].2.trim().len() + 1;
        alloc[i] = if len <= share { len } else { share };
        room -= alloc[i];
    }
    for (i, (_, _, text)) in guides.iter().enumerate() {
        let text = text.trim();
        if text.len() < alloc[i] {
            s.push_str(&heads[i]);
            s.push_str(text);
            s.push('\n');
        } else if alloc[i] >= 400 + notes[i].len() {
            s.push_str(&heads[i]);
            s.push_str(cut(text, alloc[i] - notes[i].len()));
            s.push_str(&notes[i]);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date_from_unix(0), "1970-01-01");
        assert_eq!(date_from_unix(951_782_400), "2000-02-29");
        assert_eq!(date_from_unix(1_791_158_400), "2026-10-05");
    }

    #[test]
    fn the_prompt_stays_under_budget_with_large_guides() {
        let big = "A paragraph about debugging.\n\n".repeat(2_000);
        let guides: Vec<_> = GUIDES
            .iter()
            .map(|(u, h)| ((*u).to_owned(), (*h).to_owned(), big.clone()))
            .collect();
        let p = system_prompt(
            Path::new("/w"),
            Some("Linux"),
            Some("2026-10-05"),
            true,
            &guides,
        );
        assert!(
            estimate_tokens(&p) < BUDGET_TOKENS,
            "{}",
            estimate_tokens(&p)
        );
        for (_, h) in GUIDES {
            assert!(p.contains(&format!("# {h}")), "{h}");
        }
        assert!(p.contains("(The rest of eludite://guides/debugging is left out here.)"));
        assert!(p.contains("/w (Linux). Today is 2026-10-05."));
        let small: Vec<_> = GUIDES
            .iter()
            .map(|(u, h)| ((*u).to_owned(), (*h).to_owned(), format!("Guide for {h}.")))
            .collect();
        let p = system_prompt(Path::new("/w"), None, None, true, &small);
        for (_, h) in GUIDES {
            assert!(p.contains(&format!("# {h}\n\nGuide for {h}.")));
        }
        // A long guide yields to the short ones.
        let mixed = vec![
            (GUIDES[0].0.to_owned(), "Long".to_owned(), big.clone()),
            (
                GUIDES[1].0.to_owned(),
                "Short".to_owned(),
                "Short guide.".to_owned(),
            ),
        ];
        let p = system_prompt(Path::new("/w"), None, None, true, &mixed);
        assert!(p.contains("# Short\n\nShort guide.") && p.contains("# Long"));
        assert!(estimate_tokens(&p) < BUDGET_TOKENS);
        assert!(
            estimate_tokens(&p) > BUDGET_TOKENS - 200,
            "the room is used"
        );
        assert!(estimate_tokens(&base_prompt(Path::new("/w"), "Linux", "d", true)) < 1_000);
    }
}
