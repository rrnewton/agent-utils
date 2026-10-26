//! Comparison harness for the two Slack outbound paths.
//!
//! Dry-run (the default, and the only mode CI runs): score how the
//! `converted` path's mrkdwn preserves each Markdown sample — blank lines,
//! headings, links, tables, task boxes and code fences — and report the
//! largest part the 4,000-byte cap produces. No message is posted.
//!
//! Live mode posts each sample by both paths to ONE test channel and reads
//! back what Slack stores. It runs only with all of `--live`, `--channel=C…`
//! and `SLACK_FORMAT_COMPARE_LIVE=1`, so nothing posts by accident and
//! nothing posts to a non-test channel without an operator saying so first.

use std::process::ExitCode;

use chat_subscription_slack::mrkdwn::{convert_and_split, convert_markdown};

struct Score {
    name: String,
    blank_lines_source: usize,
    blank_lines_converted: usize,
    headings_source: usize,
    headings_bold: usize,
    links_source: usize,
    links_slack: usize,
    table_rows_source: usize,
    table_in_code_block: bool,
    tasks_source: usize,
    tasks_converted: usize,
    fences_source: usize,
    fences_converted: usize,
    parts: usize,
    max_part_bytes: usize,
}

fn count(source: &str, needle: &str) -> usize {
    source.matches(needle).count()
}

fn score(name: &str, source: &str) -> Score {
    let converted = convert_markdown(source);
    let parts = convert_and_split(source, 4_000);
    Score {
        name: name.to_owned(),
        blank_lines_source: count(source, "\n\n"),
        blank_lines_converted: count(&converted, "\n\n"),
        headings_source: source.lines().filter(|line| line.starts_with('#')).count(),
        headings_bold: converted
            .lines()
            .filter(|line| line.starts_with('*') && line.ends_with('*') && line.len() > 2)
            .count(),
        links_source: count(source, "]("),
        links_slack: converted.matches("<http").count(),
        table_rows_source: source
            .lines()
            .filter(|line| line.trim_start().starts_with('|'))
            .count(),
        table_in_code_block: {
            let header = source
                .lines()
                .find(|line| line.trim_start().starts_with('|'))
                .and_then(|line| line.trim().trim_matches('|').split('|').next())
                .map_or("", |cell| cell.trim());
            !header.is_empty() && converted.contains("```\n") && converted.contains(header)
        },
        tasks_source: count(source, "- ["),
        tasks_converted: converted.matches('☐').count() + converted.matches('☑').count(),
        fences_source: count(source, "```"),
        fences_converted: count(&converted, "```"),
        parts: parts.len(),
        max_part_bytes: parts.iter().map(|part| part.len()).max().unwrap_or(0),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--live") {
        let channel = args
            .iter()
            .find_map(|arg| arg.strip_prefix("--channel="))
            .unwrap_or_default()
            .to_owned();
        if channel.is_empty()
            || std::env::var("SLACK_FORMAT_COMPARE_LIVE").ok().as_deref() != Some("1")
        {
            eprintln!(
                "live mode refused: pass --channel=C… (a test channel only) and set \
                 SLACK_FORMAT_COMPARE_LIVE=1; tell the coordinator before any live post"
            );
            return ExitCode::from(2);
        }
        eprintln!(
            "live mode is not run unattended: post each corpus file with \
             the vendor CLI's `slack.message send --channel-id={channel} --markdown=file://FILE --as=<APP>` and \
             with the converted mrkdwn from this harness's dry-run output via \
             `--slack-mrkdwn=file://-`, then read back with \
             the vendor CLI's `slack.message list --channel-id={channel} --columns=message_ts,content \
             --content-format=text` and compare blank lines, tables, headings and links"
        );
        return ExitCode::from(2);
    }
    let files: Vec<&String> = args.iter().filter(|arg| !arg.starts_with("--")).collect();
    if files.is_empty() {
        eprintln!("usage: slack_format_compare FILE.md [FILE.md ...] [--live --channel=C…]");
        return ExitCode::from(2);
    }
    println!("| sample | blank lines | headings | links | table in code block | tasks | fences | parts | max part bytes |");
    println!("|---|---|---|---|---|---|---|---|---|");
    let mut all_ok = true;
    for file in files {
        let source = match std::fs::read_to_string(file) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("{file}: {error}");
                return ExitCode::from(1);
            }
        };
        let score = score(file, &source);
        let ok = score.blank_lines_converted >= score.blank_lines_source
            && score.headings_bold >= score.headings_source
            && score.links_slack >= score.links_source
            && score.tasks_converted >= score.tasks_source
            && score.fences_converted >= score.fences_source
            && score.max_part_bytes <= 4_000
            && (score.table_rows_source == 0 || score.table_in_code_block);
        all_ok &= ok;
        println!(
            "| {} | {}/{} | {}/{} | {}/{} | {} | {}/{} | {}/{} | {} | {} |",
            score.name,
            score.blank_lines_converted,
            score.blank_lines_source,
            score.headings_bold,
            score.headings_source,
            score.links_slack,
            score.links_source,
            score.table_in_code_block,
            score.tasks_converted,
            score.tasks_source,
            score.fences_converted,
            score.fences_source,
            score.parts,
            score.max_part_bytes,
        );
        println!(
            "\nConverted output for {}:\n\n{}",
            score.name,
            convert_markdown(&source)
        );
    }
    if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
