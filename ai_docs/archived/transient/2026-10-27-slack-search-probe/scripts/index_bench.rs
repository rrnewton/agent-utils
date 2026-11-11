//! Index size, build time, incremental update and query time for substring search over a message
//! corpus: SQLite FTS5 trigram (as the vibe-talk cache uses it, and with remove_diacritics), a
//! plain table (the baseline), and tantivy with a 3-gram tokenizer. The numbers in §10 of
//! `ai_docs/transient/2026-10-25-server-side-search/README.md` came from this file, built as a
//! scratch crate OUTSIDE the repository with:
//!
//! ```toml
//! [dependencies]
//! rusqlite = { version = "=0.40.2", default-features = false, features = ["bundled"] }
//! serde_json = "1"
//! tantivy = { version = "0.27", optional = true }
//! [features]
//! tantivy = ["dep:tantivy"]
//! ```
//!
//! Usage: search-bench CORPUS.jsonl ROWS OUTDIR   (CORPUS is ground_truth.py's output)
//!        search-bench folding                    (what FTS5's own folding matches)
use std::time::Instant;

fn dir_size(path: &std::path::Path) -> u64 {
    if path.is_file() {
        return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    std::fs::read_dir(path)
        .map(|entries| entries.flatten().map(|e| dir_size(&e.path())).sum())
        .unwrap_or(0)
}

fn texts(corpus: &str, rows: usize) -> Vec<String> {
    let base: Vec<String> = std::fs::read_to_string(corpus)
        .expect("corpus")
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("json");
            format!(
                "{}\n{}",
                v["author"].as_str().unwrap_or(""),
                v["text"].as_str().unwrap_or("")
            )
            .to_lowercase()
        })
        .collect();
    let words: Vec<&str> = base.iter().flat_map(|t| t.split_whitespace()).collect();
    let mut seed = 7_u64;
    let mut next = move |n: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize % n
    };
    (0..rows)
        .map(|i| {
            let mut t = base[i % base.len()].clone();
            for _ in 0..4 {
                t.push(' ');
                t.push_str(words[next(words.len())]);
            }
            t.push_str(&format!(" u{i}x"));
            if t.len() > 8192 {
                let mut end = 8192;
                while !t.is_char_boundary(end) {
                    end -= 1;
                }
                t.truncate(end);
            }
            t
        })
        .collect()
}

fn sqlite(name: &str, tokenize: Option<&str>, texts: &[String], out: &std::path::Path, terms: &[&str]) {
    let path = out.join(format!("{name}.sqlite3"));
    let _ = std::fs::remove_file(&path);
    let c = rusqlite::Connection::open(&path).expect("open");
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;").expect("pragma");
    c.execute_batch("CREATE TABLE message (row_id INTEGER PRIMARY KEY, search_text TEXT NOT NULL);").expect("t");
    if let Some(tok) = tokenize {
        let (tok, detail) = tok.split_once('|').unwrap_or((tok, "full"));
        c.execute_batch(&format!(
            "CREATE VIRTUAL TABLE message_fts USING fts5(search_text, content='message', content_rowid='row_id', tokenize='{tok}', detail={detail});
             CREATE TRIGGER ai AFTER INSERT ON message BEGIN INSERT INTO message_fts(rowid, search_text) VALUES (new.row_id, new.search_text); END;"
        )).expect("fts");
    }
    let (bulk, more) = texts.split_at(texts.len() - 1000);
    let t = Instant::now();
    c.execute_batch("BEGIN").unwrap();
    {
        let mut ins = c.prepare("INSERT INTO message (search_text) VALUES (?1)").unwrap();
        for text in bulk {
            ins.execute([text]).unwrap();
        }
    }
    c.execute_batch("COMMIT").unwrap();
    let build = t.elapsed().as_secs_f64();
    let t = Instant::now();
    for chunk in more.chunks(50) {
        c.execute_batch("BEGIN").unwrap();
        let mut ins = c.prepare("INSERT INTO message (search_text) VALUES (?1)").unwrap();
        for text in chunk {
            ins.execute([text]).unwrap();
        }
        drop(ins);
        c.execute_batch("COMMIT").unwrap();
    }
    let incremental = t.elapsed().as_secs_f64();
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    let size = dir_size(&path);
    let mut q = Vec::new();
    for term in terms {
        let t = Instant::now();
        let like = tokenize.is_some_and(|t| t.contains('|'));
        let n: i64 = if like && term.chars().count() >= 3 {
            c.query_row(
                "SELECT COUNT(*) FROM message_fts WHERE search_text LIKE ?1",
                [format!("%{term}%")],
                |r| r.get(0),
            )
            .unwrap()
        } else if tokenize.is_some() && term.chars().count() >= 3 {
            c.query_row(
                "SELECT COUNT(*) FROM message_fts WHERE message_fts MATCH ?1",
                [format!("\"{term}\"")],
                |r| r.get(0),
            )
            .unwrap()
        } else {
            c.query_row("SELECT COUNT(*) FROM message WHERE instr(search_text, ?1) > 0", [term], |r| r.get(0)).unwrap()
        };
        q.push(format!("{term}={n} in {:.2}ms", t.elapsed().as_secs_f64() * 1000.0));
    }
    println!(
        "{{\"engine\":\"{name}\",\"rows\":{},\"bytes\":{size},\"build_s\":{build:.2},\"incremental_1000_in_50s_s\":{incremental:.2},\"queries\":{:?}}}",
        texts.len(),
        q
    );
}

#[cfg(feature = "tantivy")]
fn tantivy_bench(texts: &[String], out: &std::path::Path, terms: &[&str]) {
    use tantivy::collector::Count;
    use tantivy::query::QueryParser;
    use tantivy::schema::{IndexRecordOption, Schema, TextFieldIndexing, TextOptions};
    use tantivy::tokenizer::{AsciiFoldingFilter, LowerCaser, NgramTokenizer, TextAnalyzer};
    use tantivy::{doc, Index};
    let path = out.join("tantivy");
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let mut builder = Schema::builder();
    let indexing = TextFieldIndexing::default()
        .set_tokenizer("tri")
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let field = builder.add_text_field("text", TextOptions::default().set_indexing_options(indexing));
    let index = Index::create_in_dir(&path, builder.build()).unwrap();
    index.tokenizers().register(
        "tri",
        TextAnalyzer::builder(NgramTokenizer::new(3, 3, false).unwrap())
            .filter(LowerCaser)
            .filter(AsciiFoldingFilter)
            .build(),
    );
    let (bulk, more) = texts.split_at(texts.len() - 1000);
    let t = Instant::now();
    let mut writer = index.writer(200_000_000).unwrap();
    for text in bulk {
        writer.add_document(doc!(field => text.as_str())).unwrap();
    }
    writer.commit().unwrap();
    let build = t.elapsed().as_secs_f64();
    let t = Instant::now();
    for chunk in more.chunks(50) {
        for text in chunk {
            writer.add_document(doc!(field => text.as_str())).unwrap();
        }
        writer.commit().unwrap();
    }
    let incremental = t.elapsed().as_secs_f64();
    writer.wait_merging_threads().unwrap();
    let size = dir_size(&path);
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();
    let parser = QueryParser::for_index(&index, vec![field]);
    let mut q = Vec::new();
    for term in terms {
        let t = Instant::now();
        let query = parser.parse_query(&format!("\"{term}\"")).unwrap();
        let n = searcher.search(&query, &Count).unwrap();
        q.push(format!("{term}={n} in {:.2}ms", t.elapsed().as_secs_f64() * 1000.0));
    }
    println!(
        "{{\"engine\":\"tantivy-3gram\",\"rows\":{},\"bytes\":{size},\"build_s\":{build:.2},\"incremental_1000_in_50s_s\":{incremental:.2},\"queries\":{:?}}}",
        texts.len(),
        q
    );
}

fn folding() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    for (name, tok) in [("trigram", "trigram"), ("remove_diacritics=1", "trigram remove_diacritics 1"), ("remove_diacritics=2", "trigram remove_diacritics 2")] {
        c.execute_batch(&format!("DROP TABLE IF EXISTS f; CREATE VIRTUAL TABLE f USING fts5(x, tokenize='{tok}');
            INSERT INTO f VALUES ('Meet at the Café in Łódź; ΟΔΟΣ ΟΣΑ; Straße; naïve; ﬁnal; İstanbul');")).unwrap();
        let mut found = Vec::new();
        for q in ["cafe", "café", "CAFÉ", "lodz", "łódź", "οδος", "οσα", "strasse", "naive", "final", "istanbul"] {
            let n: i64 = c.query_row("SELECT COUNT(*) FROM f WHERE f MATCH ?1", [format!("\"{q}\"")], |r| r.get(0)).unwrap();
            found.push(format!("{q}:{n}"));
        }
        println!("{{\"folding\":\"{name}\",\"found\":{found:?}}}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && args[1] == "folding" {
        folding();
        return;
    }
    let rows: usize = args[2].parse().unwrap();
    let out = std::path::PathBuf::from(&args[3]);
    std::fs::create_dir_all(&out).unwrap();
    let texts = texts(&args[1], rows);
    let bytes: usize = texts.iter().map(String::len).sum();
    println!("{{\"rows\":{rows},\"text_bytes\":{bytes}}}");
    let rare = format!("u{}x", rows / 2);
    let terms = ["coordinator", "gate", "andoff", rare.as_str(), "the"];
    // Ground truth by plain substring, for checking each engine's counts.
    let truth: Vec<String> = terms
        .iter()
        .map(|t| format!("{t}={}", texts.iter().filter(|x| x.contains(t)).count()))
        .collect();
    println!("{{\"substring_truth\":{truth:?}}}");
    sqlite("plain", None, &texts, &out, &terms);
    sqlite("trigram", Some("trigram"), &texts, &out, &terms);
    sqlite("trigram-nodiacritics", Some("trigram remove_diacritics 1"), &texts, &out, &terms);
    sqlite("trigram-detail-column", Some("trigram|column"), &texts, &out, &terms);
    sqlite("trigram-detail-none", Some("trigram|none"), &texts, &out, &terms);
    #[cfg(feature = "tantivy")]
    tantivy_bench(&texts, &out, &terms);
}
